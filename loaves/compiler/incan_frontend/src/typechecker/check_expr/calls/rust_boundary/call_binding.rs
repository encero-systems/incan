//! Rust call argument binding and Rust method and function call validation.

use super::*;

impl TypeChecker {
    /// Contextualize Rust arguments from inspected signatures, including noncapturing boxed callbacks. Executable
    /// boxing still requires its separately retained boundary operation.
    pub(super) fn rust_params_as_callable_params(
        &self,
        params: &[incan_lang::interop::RustParam],
        owner_path: &str,
    ) -> Vec<CallableParam> {
        let params: Vec<CallableParam> = params
            .iter()
            .map(|param| {
                let ty = self
                    .rust_boxed_callable_signature(&param.type_display)
                    .unwrap_or_else(|| {
                        self.resolved_param_type_from_rust_display_for_owner_path(
                            param.type_display.as_str(),
                            owner_path,
                        )
                    });
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    surface_type_name: None,
                    kind: ParamKind::Normal,
                    has_default: false,
                    is_partial_preset: false,
                    is_mut: false,
                }
            })
            .collect();
        params
    }

    /// Record inspected Rust parameter types so codegen can emit the same borrow shape the typechecker accepted.
    pub(in crate::typechecker) fn record_rust_call_site_params(
        &mut self,
        span: Span,
        params: &[incan_lang::interop::RustParam],
        owner_path: &str,
    ) {
        let params: Vec<CallableParam> = params
            .iter()
            .map(|param| {
                let ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                    param.type_display.as_str(),
                    owner_path,
                );
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    surface_type_name: None,
                    kind: ParamKind::Normal,
                    has_default: false,
                    is_partial_preset: false,
                    is_mut: false,
                }
            })
            .collect();
        // An inspected Rust signature is the boundary authority, including by-value shapes such as `String` that the
        // emitter cannot safely infer from the argument alone. Preserve the whole selected parameter list rather than
        // retaining only explicit borrows and later falling back to a name/type heuristic.
        self.type_info.record_call_site_callable_params_exact(span, &params);
    }

    /// Bind Incan call arguments to a Rust function signature.
    pub(super) fn bind_rust_call_args<'a>(
        &mut self,
        callable_display: &str,
        params: &'a [RustParam],
        args: &'a [CallArg],
        arg_types: &'a [ResolvedType],
        span: Span,
    ) -> Vec<RustCallArgBinding<'a>> {
        let has_keyword_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::Named(_, _) | CallArg::KeywordUnpack(_)));
        let has_unpack_args = args
            .iter()
            .any(|arg| matches!(arg, CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_)));
        if !has_keyword_args || has_unpack_args {
            return self.bind_positional_rust_call_args(callable_display, params, args, arg_types, span);
        }

        let mut params_by_name = std::collections::HashMap::new();
        for (idx, param) in params.iter().enumerate() {
            if let Some(name) = param.name.as_deref() {
                params_by_name.insert(name, idx);
            }
        }

        let mut bound_spans: Vec<Option<Span>> = vec![None; params.len()];
        let mut named_seen: std::collections::HashMap<&str, Span> = std::collections::HashMap::new();
        let mut positional_index = 0usize;
        let mut unexpected_positional = 0usize;
        let mut bindings = Vec::new();

        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            let arg_span = Self::call_arg_expr(arg).span;
            match arg {
                CallArg::Positional(_) => {
                    if positional_index >= params.len() {
                        unexpected_positional += 1;
                        continue;
                    }
                    let param = &params[positional_index];
                    if let Some(bound_span) = bound_spans[positional_index] {
                        let name = param.name.as_deref().unwrap_or("<positional>");
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            name,
                            bound_span,
                            arg_span,
                        ));
                    } else {
                        bound_spans[positional_index] = Some(arg_span);
                        bindings.push(RustCallArgBinding {
                            arg,
                            arg_ty,
                            param,
                            parameter_index: positional_index,
                        });
                    }
                    positional_index += 1;
                }
                CallArg::Named(name, _) => {
                    if let Some(first_span) = named_seen.insert(name.node.as_str(), name.span) {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            &name.node,
                            first_span,
                            name.span,
                        ));
                    }
                    let Some(param_index) = params_by_name.get(name.node.as_str()).copied() else {
                        self.errors.push(errors::unknown_keyword_argument(
                            callable_display,
                            &name.node,
                            name.span,
                        ));
                        continue;
                    };
                    if let Some(first_span) = bound_spans[param_index] {
                        self.errors.push(errors::duplicate_call_argument(
                            callable_display,
                            &name.node,
                            first_span,
                            name.span,
                        ));
                        continue;
                    }
                    let param = &params[param_index];
                    bound_spans[param_index] = Some(arg_span);
                    bindings.push(RustCallArgBinding {
                        arg,
                        arg_ty,
                        param,
                        parameter_index: param_index,
                    });
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {}
            }
        }

        self.report_unbound_rust_call_args(
            callable_display,
            params,
            &bound_spans,
            unexpected_positional,
            args.len(),
            span,
        );
        bindings
    }

    /// Bind the position-only Rust path after requiring its complete inspected arity. The selected slot travels with
    /// each argument so later signature specialization cannot change its original callback predicate.
    pub(super) fn bind_positional_rust_call_args<'a>(
        &mut self,
        callable_display: &str,
        params: &'a [RustParam],
        args: &'a [CallArg],
        arg_types: &'a [ResolvedType],
        span: Span,
    ) -> Vec<RustCallArgBinding<'a>> {
        if arg_types.len() != params.len() {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                arg_types.len(),
                span,
            ));
            return Vec::new();
        }
        args.iter()
            .zip(arg_types.iter())
            .zip(params.iter().enumerate())
            .map(|((arg, arg_ty), (parameter_index, param))| RustCallArgBinding {
                arg,
                arg_ty,
                param,
                parameter_index,
            })
            .collect()
    }

    /// Report extra positional arguments and every unbound inspected parameter after named binding completes. This
    /// preserves the original arity and missing-keyword diagnostics without changing which arguments own slots.
    pub(super) fn report_unbound_rust_call_args(
        &mut self,
        callable_display: &str,
        params: &[RustParam],
        bound_spans: &[Option<Span>],
        unexpected_positional: usize,
        argument_count: usize,
        span: Span,
    ) {
        if unexpected_positional > 0 {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                params.len() + unexpected_positional,
                span,
            ));
        }

        self.refuse_missing_rust_call_parameters(callable_display, params, bound_spans, argument_count, span);
    }

    /// Emit missing Rust parameter diagnostics after every argument has bound. Named parameters retain individual
    /// errors; any missing unnamed parameter adds the existing call-wide arity error using the source arg count.
    pub(super) fn refuse_missing_rust_call_parameters(
        &mut self,
        callable_display: &str,
        params: &[RustParam],
        bound_spans: &[Option<Span>],
        source_arg_count: usize,
        span: Span,
    ) {
        let mut missing_unnamed_param = false;
        for (idx, param) in params.iter().enumerate() {
            if bound_spans[idx].is_some() {
                continue;
            }
            if let Some(name) = param.name.as_deref() {
                self.errors
                    .push(errors::missing_required_argument(callable_display, name, span));
            } else {
                missing_unnamed_param = true;
            }
        }
        if missing_unnamed_param {
            self.errors.push(errors::builtin_arity(
                callable_display,
                params.len(),
                source_arg_count,
                span,
            ));
        }
    }

    /// Return whether a lookup-style Rust method should preserve the probe argument's emitted shape.
    pub(super) fn rust_lookup_probe_boundary_match(&self, arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        let ResolvedType::Ref(inner) = target_ty else {
            return false;
        };
        match arg_ty {
            ResolvedType::Str | ResolvedType::FrozenStr => {
                matches!(
                    inner.as_ref(),
                    ResolvedType::Str | ResolvedType::RustPath(_) | ResolvedType::TypeVar(_)
                )
            }
            ResolvedType::Bytes | ResolvedType::FrozenBytes => {
                matches!(inner.as_ref(), ResolvedType::RustPath(_) | ResolvedType::TypeVar(_))
            }
            _ => false,
        }
    }

    /// Return whether an argument can cross a Rust boundary.
    #[cfg(test)]
    pub(in crate::typechecker) fn rust_arg_matches_boundary(&self, arg_ty: &ResolvedType, rust_param_ty: &str) -> bool {
        !matches!(
            self.rust_arg_boundary_match(arg_ty, rust_param_ty),
            RustArgBoundaryMatch::NoMatch
        )
    }

    /// Return whether a call argument typed without context is a bare Rust owner that the parameter's generic
    /// spelling of the same owner would specialize.
    ///
    /// Arguments are typed before the callee resolves, so a `Self`-returning constructor such as `Box.new(v)`
    /// arrives as the bare owner `std::boxed::Box` while the parameter reads `Box<T>`. Re-checking that one
    /// argument against its parameter lets the receiver reconcile — the same contextual path literals and closures
    /// already take. Projected source collections also carry their parameter expectation into the nested constructor.
    /// An open fixed array similarly needs its parent's exact array before callable const binders can close. Native
    /// owners are compared in the ancestral namespace so every re-export spelling matches.
    pub(super) fn rust_call_arg_wants_owner_expectation(arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        if arg_ty == target_ty {
            return false;
        }
        if let (ResolvedType::Generic(actual, _), ResolvedType::Generic(expected, _)) = (arg_ty, target_ty) {
            return actual == expected;
        }
        if Self::rust_call_arg_wants_fixed_array_expectation(arg_ty, target_ty) {
            return true;
        }
        // The uncontextual type may already carry the owner's own unresolved parameters (`RefCell<T>` from a
        // `RefCell<T>`-returning `new`), so compare owners by base, not whole display.
        let arg_owner = match arg_ty {
            ResolvedType::RustPath(path) | ResolvedType::Named(path) => path.split('<').next().unwrap_or(path),
            _ => return false,
        };
        let ResolvedType::RustPath(target) = target_ty else {
            return false;
        };
        let Some((base, _)) = target.split_once('<') else {
            return false;
        };
        incan_lang::interop::ancestral_rust_path(base) == incan_lang::interop::ancestral_rust_path(arg_owner)
    }

    /// Return whether an open fixed-array call result needs the enclosing Rust parameter's exact array expectation.
    pub(super) fn rust_call_arg_wants_fixed_array_expectation(arg_ty: &ResolvedType, target_ty: &ResolvedType) -> bool {
        let (ResolvedType::RustPath(actual), ResolvedType::RustPath(expected)) = (arg_ty, target_ty) else {
            return false;
        };
        let fixed_array = |display: &str| {
            let display = display.trim().strip_prefix("rust::").unwrap_or(display.trim());
            syn::parse_str::<syn::Type>(display).is_ok_and(|ty| matches!(ty, syn::Type::Array(_)))
        };
        fixed_array(actual) && fixed_array(expected) && actual != expected
    }

    /// Validate a Rust method call (`receiver.method(...)`) against metadata and record required arg coercions.
    ///
    /// The receiver is already validated by access resolution; this function validates only post-receiver parameters.
    pub(in crate::typechecker) fn validate_rust_method_call(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
    ) -> ResolvedType {
        self.validate_rust_method_call_with_expected(
            callable_display,
            sig,
            args,
            arg_types,
            preserves_lookup_arg_shape,
            span,
            None,
        )
    }

    /// Validate a direct Rust function call (`rust::path::item(...)`) and record boundary coercions.
    #[cfg(test)]
    pub(in crate::typechecker::check_expr) fn validate_rust_function_call(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.validate_rust_function_call_with_expected(path, sig, args, span, None)
    }

    /// Validate a Rust function from its metadata, instantiate its parameters from inputs and expected output, and
    /// retain the selected callee for Body IR when validation succeeds. Unsafe signatures are refused independently
    /// of checked C acknowledgment regions: RFC 005 provides no Rust safety opt-in.
    pub(in crate::typechecker::check_expr) fn validate_rust_function_call_with_expected(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        self.validate_rust_function_call_with_type_arguments(path, sig, &[], args, span, expected_return_ty)
    }

    /// Validate a directly imported Rust function with explicit Incan type arguments.
    ///
    /// Inspected Rust binders are authoritative for arity. Written arguments seed those binders before the ordinary
    /// boundary matcher specializes parameters and the return type, including binders that occur only in the result.
    pub(in crate::typechecker::check_expr) fn validate_rust_function_call_with_type_arguments(
        &mut self,
        path: &str,
        sig: &RustFunctionSig,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let errors_before = self.errors.len();
        let mut explicit_bindings = HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != sig.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    path,
                    &sig.type_params,
                    &Self::written_type_args(explicit_type_args),
                    span,
                ));
            } else {
                explicit_bindings.extend(
                    sig.type_params.iter().cloned().zip(
                        explicit_type_args
                            .iter()
                            .map(|argument| self.resolve_type_checked(argument)),
                    ),
                );
            }
        }
        if sig.is_unsafe {
            self.check_call_args(args);
            self.errors.push(errors::rust_unsafe_call_not_supported(path, span));
            return ResolvedType::Unknown;
        }
        if sig.is_async {
            self.type_info.rust.async_call_realizations.insert(span);
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors.push(errors::async_call_without_await(path, span));
        }
        let expected_params = self.rust_params_as_callable_params(&sig.params, path);
        let arg_types = self.check_call_arg_types_for_params(args, &expected_params);
        // A closure passed to a callable-bounded binder takes its parameter types from the bound, as for a method.
        let arg_types = self.contextualize_rust_bound_closures(sig, path, args, &arg_types, expected_return_ty);
        self.record_rust_call_site_params(span, &sig.params, path);
        let binding_errors_before = self.errors.len();
        self.bind_rust_call_args(path, &sig.params, args, &arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, path, span);
        }

        let inference_parameters = Self::rust_signature_inference_parameters(sig);
        let mut callable_returns =
            self.infer_rust_signature_bindings(sig, path, &inference_parameters, args, &arg_types, expected_return_ty);
        callable_returns.extend(explicit_bindings);
        self.validate_rust_scalar_predicates(sig, &callable_returns, span);
        self.record_inferred_rust_arguments(path, sig, &callable_returns, span);
        let effective_sig = self.specialize_inferred_rust_signature(sig, path, &callable_returns);
        self.record_rust_call_site_params(span, &effective_sig.params, path);
        let bindings = self.bind_rust_call_args(path, &effective_sig.params, args, &arg_types, span);
        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let param = binding.param;
            if let Some(declared) = sig.params.get(binding.parameter_index)
                && self.validate_rust_bound_callable_argument(
                    sig,
                    &declared.type_display,
                    &callable_returns,
                    arg_expr,
                    binding.arg_ty,
                )
            {
                continue;
            }
            let target_ty =
                self.resolved_rust_boundary_target_from_param_display_for_owner_path(param.type_display.as_str(), path);
            // This path had no general second-pass argument check before const-array inference. Limit the new
            // contextual pass to the open array relation that requires it: rechecking arbitrary nested calls can
            // consume their retained native selection twice.
            let contextual_arg_ty = if matches!(arg_expr.node, Expr::Call(..) | Expr::MethodCall(..))
                && Self::rust_call_arg_wants_fixed_array_expectation(binding.arg_ty, &target_ty)
            {
                Some(self.check_expr_with_expected(arg_expr, Some(&target_ty)))
            } else {
                None
            };
            let arg_ty = contextual_arg_ty.as_ref().unwrap_or(binding.arg_ty);
            let instantiated = sig.params.get(binding.parameter_index).map(|declared| {
                self.instantiate_rust_signature_type(
                    &self.native_rust_signature_type(&declared.type_display, path),
                    &inference_parameters,
                    &callable_returns,
                )
            });
            self.validate_rust_boundary_value_with_target(
                path,
                param.type_display.as_str(),
                arg_expr,
                arg_ty,
                false,
                instantiated.as_ref(),
            );
        }

        let ret = self.resolved_rust_call_type_from_sig(&effective_sig, path, span);
        let ret = self.instantiate_rust_signature_type(&ret, &inference_parameters, &callable_returns);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        if self.errors.len() == errors_before && Self::rust_signature_const_parameters_are_bound(sig, &callable_returns)
        {
            self.retain_checked_rust_function(path, sig, &callable_returns, span);
        }
        self.native_borrowed_source_result(span, ret)
    }
}
