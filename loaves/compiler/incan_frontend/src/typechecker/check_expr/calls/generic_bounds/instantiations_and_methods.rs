//! Call-site monomorphs, constructor and factory instantiations, and generic method calls.

use super::*;

impl TypeChecker {
    /// Record ordered explicit or inferred call-site arguments after every declaration type parameter has a resolved
    /// binding.
    pub(super) fn record_call_site_monomorph_if_complete(
        &mut self,
        call_span: Span,
        type_params: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        self.retain_body_ir_factory_arguments(call_span, type_params, bindings);
        let mut out: Vec<ResolvedType> = Vec::new();
        for p in type_params {
            let Some(ty) = bindings.get(p) else {
                return;
            };
            if matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer) {
                return;
            }
            out.push(ty.clone());
        }
        self.type_info
            .calls
            .call_site_monomorph_type_args
            .insert(call_span, out);
    }

    /// Preserve checked partial factory inference before the legacy complete-only record is considered.
    /// Only the exact explicitly checked factory span participates. The selected single-function identity and the
    /// decorator application must subsequently validate this provisional record before Body IR can consume it.
    pub(super) fn retain_body_ir_factory_arguments(
        &mut self,
        call_span: Span,
        parameters: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        if self.body_ir_decorator_factory_span != Some(call_span) {
            return;
        }
        let arguments = parameters
            .iter()
            .map(|parameter| {
                bindings
                    .get(parameter)
                    .cloned()
                    .unwrap_or_else(|| ResolvedType::TypeVar(parameter.clone()))
            })
            .collect();
        self.type_info.declarations.body_ir_decorator_applications.insert(
            call_span,
            BodyIrDecoratorApplication {
                callable_type: ResolvedType::Unknown,
                type_bindings: std::collections::HashMap::new(),
                factory_type_args: arguments,
            },
        );
    }

    /// Retain concrete inferred call arguments from checked constructor facts without changing legacy contextual types.
    pub(super) fn retain_constructor_call_instantiation(
        &mut self,
        call_span: Span,
        type_params: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        return_type: &ResolvedType,
    ) {
        let mut concrete = bindings.clone();
        concrete.retain(|_, ty| first_open_type_param(ty, type_params).is_none());
        let mut has_constructor_fact = false;
        for (expr, parameter) in arguments {
            if let Some(actual) = self.type_info.calls.inferred_constructor_types.get(&expr.span).cloned() {
                has_constructor_fact = true;
                self.infer_type_param_bindings(parameter, &actual, &mut concrete);
            }
        }
        if !has_constructor_fact || type_params.iter().any(|parameter| !concrete.contains_key(parameter)) {
            return;
        }
        self.record_call_site_monomorph_if_complete(call_span, type_params, &concrete);
        self.type_info
            .calls
            .inferred_call_result_types
            .insert(call_span, substitute_resolved_type(return_type, &concrete));
    }

    /// Close one executable call from Body IR-only constructor facts, preserving every legacy-read call table.
    ///
    /// This uses the same checked structural binding relation as ordinary call inference, but only settled bindings
    /// satisfying the declared bounds cross the Body IR boundary. Missing constructor or parameter facts leave the
    /// call's existing representation unchanged; native lowering does not infer arguments from runtime operands.
    pub(super) fn retain_body_ir_constructor_call_instantiation(
        &mut self,
        span: Span,
        parameters: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        returned: &ResolvedType,
        bounds: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
    ) {
        let mut concrete = bindings
            .iter()
            .filter(|(_, ty)| Self::is_settled_type_argument(ty))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        let mut has_constructor = false;
        for (expression, parameter) in arguments {
            if let Some(actual) = self.type_info.calls.body_ir_constructor_types.get(&expression.span) {
                has_constructor = true;
                self.infer_type_param_bindings(parameter, actual, &mut concrete);
            }
        }
        if !has_constructor
            || parameters.iter().any(|parameter| {
                concrete
                    .get(parameter)
                    .is_none_or(|ty| !Self::is_settled_type_argument(ty))
            })
        {
            return;
        }
        for (parameter, requirements) in bounds {
            let Some(actual) = concrete.get(parameter) else {
                return;
            };
            if requirements
                .iter()
                .any(|bound| !self.type_satisfies_explicit_bound_info(actual, bound, &concrete))
            {
                return;
            }
        }
        let ordered = parameters
            .iter()
            .filter_map(|parameter| concrete.get(parameter).cloned())
            .collect();
        self.type_info.calls.body_ir_call_type_arguments.insert(span, ordered);
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert(span, substitute_resolved_type(returned, &concrete));
    }

    /// Seed owner type-parameter bindings from the concrete receiver type.
    pub(super) fn receiver_type_param_bindings(
        &self,
        receiver_ty: &ResolvedType,
    ) -> std::collections::HashMap<String, ResolvedType> {
        let (type_name, type_args) = match receiver_ty {
            ResolvedType::Generic(name, args) => (name, args.as_slice()),
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                return self.receiver_type_param_bindings(inner);
            }
            _ => return std::collections::HashMap::new(),
        };
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return std::collections::HashMap::new();
        };
        let type_params = match info {
            TypeInfo::Model(model) => model.type_params.as_slice(),
            TypeInfo::Class(class) => class.type_params.as_slice(),
            TypeInfo::Enum(en) => en.type_params.as_slice(),
            TypeInfo::Newtype(newtype) => newtype.type_params.as_slice(),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return std::collections::HashMap::new(),
        };
        type_params
            .iter()
            .zip(type_args.iter())
            .map(|(param, arg)| (param.clone(), arg.clone()))
            .collect()
    }

    /// Apply type bindings to callable parameters while preserving names, default markers, and parameter kind.
    pub(in crate::typechecker::check_expr) fn substitute_callable_params(
        params: &[CallableParam],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> Vec<CallableParam> {
        params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: substitute_resolved_type(&param.ty, bindings),
                surface_type_name: param.surface_type_name.clone(),
                kind: param.kind,
                has_default: param.has_default,
                is_partial_preset: param.is_partial_preset,
                is_mut: param.is_mut,
            })
            .collect()
    }

    /// Type-check a resolved [`MethodInfo`] for a call site that may include explicit bracketed type arguments (RFC
    /// 054).
    ///
    /// Pipeline role: invoked from [`TypeChecker::resolve_named_method`] after a concrete method has been chosen
    /// (inherent or trait).
    ///
    /// This runs the full generic call-site path for methods:
    /// - Validates arity when `explicit_type_args` is nonempty.
    /// - Builds a partial substitution map (skipping [`ResolvedType::CallSiteInfer`] for `_` slots), substitutes
    ///   call-site `Self` via [`TypeChecker::method_types_substituting_call_site_self`], then uses the optional
    ///   expected return type to bind still-open method type parameters before argument checking.
    /// - Validates value arguments against the specialized formals, then runs [`Self::infer_type_param_bindings`] so
    ///   remaining type parameters are filled from argument types.
    /// - Unifies the bindings arguments give one type parameter, retypes literal arguments at the instantiation and
    ///   refuses arguments whose bindings do not unify, as a function call does (#1561).
    /// - Enforces explicit `with` bounds and the hashed type parameters (#1758) against the bindings closed by literal
    ///   arguments, requires every method type parameter to be concretely bound when brackets were present, and records
    ///   `TypeCheckInfo::calls.call_site_monomorph_type_args` for lowering.
    ///
    /// # Parameters
    ///
    /// - `method`: Method name (for diagnostics).
    /// - `method_info`: Owned selected [`MethodInfo`] whose checked receiver and binders determine substitution.
    /// - `explicit_type_args`: AST types inside `[...]` before `(`; empty if the call omitted brackets.
    /// - `args`: Call arguments. The selected method parameters are threaded back into these expressions so inline
    ///   collection literals can adopt contextual element types.
    /// - `_arg_types`: Argument types from the pre-selection pass. Method validation recomputes them after the final
    ///   parameter list is known.
    /// - `call_site_span`: Span of the whole `MethodCall` expression (monomorph snapshot key).
    /// - `receiver_ty`: Resolved type of the receiver expression.
    ///
    /// # Returns
    ///
    /// The method’s return type after substituting inferred bindings into `return_type` (post–`Self` substitution).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::typechecker::check_expr) fn check_generic_method_call(
        &mut self,
        method: &str,
        method_info: MethodInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        _arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let errors_before_call = self.errors.len();
        let (mut type_bindings, explicit_arity_ok) = self.initial_generic_method_call_bindings(
            method,
            &method_info,
            explicit_type_args,
            call_site_span,
            receiver_ty,
        );

        // ---- Call-site `Self`, value-arg compatibility ----
        let (mut params, mut return_type) = self.method_types_substituting_call_site_self(&method_info, receiver_ty);
        match self.ownership_call_value_result(method_info.identity.as_ref(), &return_type, call_site_span) {
            Ok(Some(value)) => return_type = value,
            Ok(None) => {}
            Err(error) => {
                self.errors.push(error);
                return ResolvedType::Unknown;
            }
        }
        match self.ownership_call_value_parameters(
            method_info.identity.as_ref(),
            &params,
            Some(receiver_ty),
            call_site_span,
        ) {
            Ok(Some(values)) => params = values,
            Ok(None) => {}
            Err(error) => {
                self.errors.push(error);
                return ResolvedType::Unknown;
            }
        }
        if let Some(expected) = expected_return_ty {
            self.infer_type_param_bindings(&return_type, expected, &mut type_bindings);
        }
        let params = Self::substitute_callable_params(&params, &type_bindings);
        let contextual_params = self.contextualize_source_callable_params(
            &params,
            args,
            &method_info.type_param_bound_details,
            &type_bindings,
        );
        let return_type = substitute_resolved_type(&return_type, &type_bindings);
        let arg_types = self.check_call_arg_types_binding_closures_last(args, &contextual_params, &type_bindings);
        self.seed_contextual_callable_type_param_bindings(&params, args, &arg_types, &mut type_bindings);
        self.validate_callable_arg_bindings(
            method,
            &contextual_params,
            args,
            &arg_types,
            &mut type_bindings,
            call_site_span,
        );
        self.refuse_non_task_arguments(
            method,
            &contextual_params,
            args,
            &arg_types,
            &method_info.type_param_bound_details,
            &mut type_bindings,
        );
        self.infer_type_param_bindings_from_source_bounds(&method_info.type_param_bound_details, &mut type_bindings);
        let arguments = Self::arguments_with_parameter_types(&params, args);
        self.unify_argument_type_bindings(&method_info.type_params, &arguments, &mut type_bindings);
        let mut instantiation =
            self.bindings_closed_by_literal_arguments(&method_info.type_params, &params, args, &type_bindings);
        if self.errors.len() == errors_before_call {
            self.close_bindings_from_literal_arguments(
                method,
                &method_info.type_params,
                &params,
                args,
                &instantiation,
                &mut type_bindings,
            );
        }
        self.refuse_unhashable_type_arguments(
            method,
            method_info.identity.as_ref(),
            &method_info.type_params,
            &instantiation,
            call_site_span,
        );
        self.settle_constructor_argument_sides(&method_info.type_params, &params, args, &mut type_bindings);
        Self::close_instantiation_from_bindings(&mut instantiation, &type_bindings, &method_info.type_params);
        if self.errors.len() == errors_before_call {
            self.refuse_conflicting_type_arguments(method, &method_info.type_params, &arguments, &type_bindings);
        }
        if self.errors.len() == errors_before_call {
            self.record_argument_destinations_at_bindings(&method_info.type_params, &arguments, &type_bindings);
        }
        if self.errors.len() == errors_before_call && explicit_type_args.is_empty() {
            self.refuse_type_params_left_open(OpenTypeParamCall {
                callee: method,
                method: true,
                declaration: method_info.identity.as_ref(),
                type_params: &method_info.type_params,
                bound_details: &method_info.type_param_bound_details,
                params: &params,
                return_type: &return_type,
                args,
                bindings: &instantiation,
                span: call_site_span,
            });
        }
        self.finish_checked_method_call(MethodCallCompletion {
            method,
            method_info,
            explicit_type_args,
            args,
            call_site_span,
            receiver_ty,
            expected_return_ty,
            errors_before_call,
            explicit_arity_ok,
            contextual_params,
            type_bindings,
            instantiation,
            arguments,
            return_type,
        })
    }

    /// Seed receiver binders before explicit method arguments, preserving partial-map inference markers and the
    /// existing arity diagnostic. Invalid explicit arity still leaves receiver-derived bindings available.
    pub(super) fn initial_generic_method_call_bindings(
        &mut self,
        method: &str,
        info: &MethodInfo,
        explicit_type_args: &[Spanned<Type>],
        span: Span,
        receiver_ty: &ResolvedType,
    ) -> (HashMap<String, ResolvedType>, bool) {
        let mut bindings = self.receiver_type_param_bindings(receiver_ty);
        let arity_ok = explicit_type_args.is_empty() || explicit_type_args.len() == info.type_params.len();
        if !explicit_type_args.is_empty() {
            if !arity_ok {
                self.errors.push(errors::explicit_type_arg_arity(
                    method,
                    &info.type_params,
                    &Self::written_type_args(explicit_type_args),
                    Self::explicit_type_argument_span(explicit_type_args, span),
                ));
            } else {
                let resolved = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect::<Vec<_>>();
                bindings.extend(type_param_subst_map_call_site(&info.type_params, &resolved));
            }
        }
        (bindings, arity_ok)
    }

    /// Retain the checked method's parameter, capturing-callable, bound and result facts after argument validation.
    /// Method-specific guards and their ordering remain distinct from function-call completion.
    pub(super) fn finish_checked_method_call(&mut self, completion: MethodCallCompletion<'_>) -> ResolvedType {
        let MethodCallCompletion {
            method,
            method_info,
            explicit_type_args,
            args,
            call_site_span,
            receiver_ty,
            expected_return_ty,
            errors_before_call,
            explicit_arity_ok,
            contextual_params,
            type_bindings,
            instantiation,
            arguments,
            return_type,
        } = completion;
        if let Some(identity) = method_info.identity.clone()
            && let ResolvedType::Named(owner) | ResolvedType::Generic(owner, _) = receiver_ty
        {
            self.record_nominal_member_identity(owner, call_site_span, identity);
        }
        let resolved_params = Self::substitute_callable_params(&contextual_params, &type_bindings);
        self.type_info
            .record_call_site_callable_params_exact(call_site_span, &resolved_params);
        if method_info.is_async {
            self.warn_if_unawaited_async_call(method, call_site_span);
        }
        self.check_capturing_call_arguments(method_info.identity.as_ref(), method, &method_info.params, args);

        self.emit_explicit_bound_errors(
            method,
            &method_info.type_param_bounds,
            &method_info.type_param_bound_details,
            &instantiation,
            Self::type_argument_origin(explicit_type_args),
            call_site_span,
        );

        // ---- Require explicit bindings and retain complete explicit or inferred method instantiations ----
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(
                method,
                &method_info.type_params,
                &type_bindings,
                call_site_span,
            );
        }
        if self.errors.len() == errors_before_call {
            self.record_call_site_monomorph_if_complete(call_site_span, &method_info.type_params, &type_bindings);
            self.retain_body_ir_newtype_method_result(&method_info, receiver_ty, args, &type_bindings, call_site_span);
            self.retain_body_ir_factory_owner(
                &method_info,
                receiver_ty,
                args,
                &type_bindings,
                expected_return_ty,
                call_site_span,
            );
            if explicit_type_args.is_empty() {
                self.retain_constructor_call_instantiation(
                    call_site_span,
                    &method_info.type_params,
                    &arguments,
                    &type_bindings,
                    &return_type,
                );
                self.retain_body_ir_constructor_call_instantiation(
                    call_site_span,
                    &method_info.type_params,
                    &arguments,
                    &type_bindings,
                    &return_type,
                    &method_info.type_param_bound_details,
                );
            }
        }

        if self.errors.len() == errors_before_call {
            let templates = self.contextual_parameter_templates(
                &resolved_params,
                method_info.identity.as_ref(),
                &method_info.params,
            );
            for ((expression, expected), (_, template)) in Self::arguments_with_parameter_types(&resolved_params, args)
                .into_iter()
                .zip(Self::arguments_with_parameter_types(&templates, args))
            {
                self.retain_contextual_literal_type(
                    expression,
                    expected,
                    method_info.identity.as_ref(),
                    Some(template),
                );
            }
            self.retain_declared_nominal_result(
                call_site_span,
                &method_info.return_type,
                &type_bindings,
                method_info.identity.as_ref(),
            );
        }
        self.type_info
            .calls
            .body_ir_call_result_types
            .get(&call_site_span)
            .cloned()
            .unwrap_or_else(|| substitute_resolved_type(&return_type, &type_bindings))
    }
}
