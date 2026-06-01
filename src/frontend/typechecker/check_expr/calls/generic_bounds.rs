//! Generic call-site inference, monomorph recording, and explicit bound validation.

use super::TypeChecker;
use crate::frontend::ast::{CallArg, Span, Spanned, Type};
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::frontend::symbols::{CallableParam, FunctionInfo, MethodInfo, ResolvedType, TypeInfo};

impl TypeChecker {
    /// Validate generic function call type arguments, contextual return bindings, value arguments, and explicit
    /// type-parameter bounds.
    pub(in crate::frontend::typechecker::check_expr::calls) fn validate_function_call(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let mut seeded_type_bindings: std::collections::HashMap<String, ResolvedType> =
            std::collections::HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != info.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    func_name,
                    info.type_params.len(),
                    explicit_type_args.len(),
                    call_span,
                ));
            } else {
                let resolved_explicit: Vec<ResolvedType> = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect();
                seeded_type_bindings = type_param_subst_map_call_site(&info.type_params, &resolved_explicit);
            }
        }
        if let Some(expected) = expected_return_ty {
            self.infer_type_param_bindings(&info.return_type, expected, &mut seeded_type_bindings);
        }
        let params_with_explicit = Self::substitute_callable_params(&info.params, &seeded_type_bindings);
        let arg_types = self.check_call_arg_types_for_params(args, &params_with_explicit);
        let mut type_bindings = seeded_type_bindings;
        self.validate_callable_arg_bindings(
            func_name,
            &params_with_explicit,
            args,
            &arg_types,
            &mut type_bindings,
            call_span,
        );
        let resolved_params = Self::substitute_callable_params(&params_with_explicit, &type_bindings);
        self.type_info
            .record_call_site_callable_params(call_span, &resolved_params);
        self.emit_explicit_bound_errors(
            func_name,
            &info.type_param_bounds,
            &info.type_param_bound_details,
            &type_bindings,
            call_span,
        );
        if info.is_async {
            self.warn_if_unawaited_async_call(func_name, call_span);
        }

        let explicit_arity_ok = explicit_type_args.is_empty() || explicit_type_args.len() == info.type_params.len();
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(func_name, &info.type_params, &type_bindings, call_span);
            self.record_call_site_monomorph_if_complete(call_span, &info.type_params, &type_bindings);
        }

        substitute_resolved_type(&info.return_type, &type_bindings)
    }

    /// Assert that call-site type parameters have been inferred.
    fn assert_call_site_type_params_inferred(
        &mut self,
        callee: &str,
        type_params: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        span: Span,
    ) {
        for p in type_params {
            let ok = match bindings.get(p) {
                Some(ty) => !matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer),
                None => false,
            };
            if !ok {
                self.errors
                    .push(errors::call_site_type_inference_unresolved(callee, p, span));
            }
        }
    }

    /// Record explicit call-site generic arguments after every type parameter has a concrete resolved type.
    fn record_call_site_monomorph_if_complete(
        &mut self,
        call_span: Span,
        type_params: &[String],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
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
            .insert((call_span.start, call_span.end), out);
    }

    /// Seed owner type-parameter bindings from the concrete receiver type.
    fn receiver_type_param_bindings(
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
    pub(in crate::frontend::typechecker::check_expr) fn substitute_callable_params(
        params: &[CallableParam],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> Vec<CallableParam> {
        params
            .iter()
            .map(|param| CallableParam {
                name: param.name.clone(),
                ty: substitute_resolved_type(&param.ty, bindings),
                kind: param.kind,
                has_default: param.has_default,
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
    /// - Enforces explicit `with` bounds, requires every method type parameter to be concretely bound when brackets
    ///   were present, and records `TypeCheckInfo::calls.call_site_monomorph_type_args` for lowering.
    ///
    /// # Parameters
    ///
    /// - `method`: Method name (for diagnostics).
    /// - `method_info`: Declared [`MethodInfo`] for that method (owned and temporarily mutated for substitution).
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
    pub(in crate::frontend::typechecker::check_expr) fn check_generic_method_call(
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
        let mut type_bindings = self.receiver_type_param_bindings(receiver_ty);
        let explicit_arity_ok =
            explicit_type_args.is_empty() || explicit_type_args.len() == method_info.type_params.len();

        // ---- RFC 054: explicit bracketed type arguments (partial map; `_` → CallSiteInfer omitted) ----
        if !explicit_type_args.is_empty() {
            if !explicit_arity_ok {
                self.errors.push(errors::explicit_type_arg_arity(
                    method,
                    method_info.type_params.len(),
                    explicit_type_args.len(),
                    call_site_span,
                ));
            } else {
                let resolved: Vec<ResolvedType> = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect();
                type_bindings.extend(type_param_subst_map_call_site(&method_info.type_params, &resolved));
            }
        }

        // ---- Call-site `Self`, value-arg compatibility ----
        let (params, return_type) = self.method_types_substituting_call_site_self(&method_info, receiver_ty);
        if let Some(expected) = expected_return_ty {
            self.infer_type_param_bindings(&return_type, expected, &mut type_bindings);
        }
        let params = Self::substitute_callable_params(&params, &type_bindings);
        let return_type = substitute_resolved_type(&return_type, &type_bindings);
        let arg_types = self.check_call_arg_types_for_params(args, &params);
        self.validate_callable_arg_bindings(method, &params, args, &arg_types, &mut type_bindings, call_site_span);
        let resolved_params = Self::substitute_callable_params(&params, &type_bindings);
        self.type_info
            .record_call_site_callable_params_exact(call_site_span, &resolved_params);
        if method_info.is_async {
            self.warn_if_unawaited_async_call(method, call_site_span);
        }

        self.emit_explicit_bound_errors(
            method,
            &method_info.type_param_bounds,
            &method_info.type_param_bound_details,
            &type_bindings,
            call_site_span,
        );

        // ---- Require concrete bindings; snapshot monomorphs for lowering when brackets were used ----
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(
                method,
                &method_info.type_params,
                &type_bindings,
                call_site_span,
            );
            self.record_call_site_monomorph_if_complete(call_site_span, &method_info.type_params, &type_bindings);
        }

        substitute_resolved_type(&return_type, &type_bindings)
    }

    /// Infer concrete type bindings for generic type parameters from a parameter/argument type pair.
    ///
    /// This walks matching container structure recursively so constructor field checks and function calls can recover
    /// bindings such as `T -> String` from shapes like `Boxed[T]` versus `Boxed[String]`.
    pub(in crate::frontend::typechecker::check_expr::calls) fn infer_type_param_bindings(
        &self,
        expected: &ResolvedType,
        actual: &ResolvedType,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        match expected {
            ResolvedType::TypeVar(name) => {
                bindings
                    .entry(name.clone())
                    .and_modify(|existing| {
                        if !self.types_compatible(actual, existing) {
                            *existing = ResolvedType::Unknown;
                        }
                    })
                    .or_insert_with(|| actual.clone());
            }
            ResolvedType::Generic(name, expected_args) => {
                if let ResolvedType::Generic(actual_name, actual_args) = actual
                    && name == actual_name
                {
                    for (e, a) in expected_args.iter().zip(actual_args.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                }
            }
            ResolvedType::Function(expected_params, expected_ret) => {
                if let ResolvedType::Function(actual_params, actual_ret) = actual {
                    for (e, a) in expected_params.iter().zip(actual_params.iter()) {
                        self.infer_type_param_bindings(&e.ty, &a.ty, bindings);
                    }
                    self.infer_type_param_bindings(expected_ret, actual_ret, bindings);
                }
            }
            ResolvedType::Tuple(expected_items) => {
                if let ResolvedType::Tuple(actual_items) = actual {
                    for (e, a) in expected_items.iter().zip(actual_items.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                }
            }
            ResolvedType::FrozenList(inner) => {
                if let ResolvedType::FrozenList(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::FrozenSet(inner) => {
                if let ResolvedType::FrozenSet(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::FrozenDict(k, v) => {
                if let ResolvedType::FrozenDict(actual_k, actual_v) = actual {
                    self.infer_type_param_bindings(k, actual_k, bindings);
                    self.infer_type_param_bindings(v, actual_v, bindings);
                }
            }
            ResolvedType::Ref(inner) => {
                if let ResolvedType::Ref(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                } else if let ResolvedType::RefMut(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            ResolvedType::RefMut(inner) => {
                if let ResolvedType::RefMut(actual_inner) = actual {
                    self.infer_type_param_bindings(inner, actual_inner, bindings);
                }
            }
            _ => {}
        }
    }

    /// Emit diagnostics when inferred concrete generic bindings violate explicit `with` bounds.
    fn emit_explicit_bound_errors(
        &mut self,
        func_name: &str,
        bounds_by_param: &std::collections::HashMap<String, Vec<String>>,
        bound_details_by_param: &std::collections::HashMap<String, Vec<crate::frontend::symbols::TypeBoundInfo>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        for (type_param, bounds) in bounds_by_param {
            let Some(actual_ty) = bindings.get(type_param) else {
                continue;
            };
            if let Some(details) = bound_details_by_param.get(type_param)
                && !details.is_empty()
            {
                for bound in details {
                    if !self.type_satisfies_explicit_bound_info(actual_ty, bound, bindings) {
                        self.errors.push(errors::generic_bound_not_satisfied(
                            func_name,
                            type_param,
                            &self.type_bound_display(bound, bindings),
                            &actual_ty.to_string(),
                            call_span,
                        ));
                    }
                }
                continue;
            }
            for bound in bounds {
                if !self.type_satisfies_explicit_bound(actual_ty, bound) {
                    self.errors.push(errors::generic_bound_not_satisfied(
                        func_name,
                        type_param,
                        bound,
                        &actual_ty.to_string(),
                        call_span,
                    ));
                }
            }
        }
    }
}
