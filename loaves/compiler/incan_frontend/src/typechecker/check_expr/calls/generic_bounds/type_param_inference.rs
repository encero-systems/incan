//! Type parameter inference from source callables and bounds, written type arguments, and bound errors.

use super::*;

impl TypeChecker {
    /// Project a generic `CallableN` bound into the contextual function type used to check a callback argument.
    pub(super) fn contextualize_source_callable_params(
        &mut self,
        params: &[CallableParam],
        args: &[CallArg],
        bound_details: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> Vec<CallableParam> {
        let closure_params = Self::closure_argument_param_indexes(args, params);
        params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let generic_name = match &param.ty {
                    ResolvedType::TypeVar(name) | ResolvedType::Named(name) => Some(name.as_str()),
                    _ => None,
                };
                let ty = if closure_params.contains_key(&index)
                    && let Some(signature) = generic_name
                        .and_then(|name| self.source_callable_signature_for_param(name, bound_details, bindings))
                {
                    signature
                } else {
                    param.ty.clone()
                };
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    surface_type_name: param.surface_type_name.clone(),
                    kind: param.kind,
                    has_default: param.has_default,
                    is_partial_preset: param.is_partial_preset,
                    is_mut: param.is_mut,
                }
            })
            .collect()
    }

    /// Map direct closure arguments to their bound normal parameter indexes.
    pub(super) fn closure_argument_param_indexes(
        args: &[CallArg],
        params: &[CallableParam],
    ) -> std::collections::HashMap<usize, Span> {
        let normal_indexes = params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| (param.kind == ParamKind::Normal).then_some(index))
            .collect::<Vec<_>>();
        let mut positional_index = 0usize;
        let mut closure_params = std::collections::HashMap::new();
        for arg in args {
            match arg {
                CallArg::Positional(expr) => {
                    if matches!(expr.node, crate::ast::Expr::Closure(_, _))
                        && let Some(param_index) = normal_indexes.get(positional_index)
                    {
                        closure_params.insert(*param_index, expr.span);
                    }
                    positional_index += 1;
                }
                CallArg::Named(name, expr) if matches!(expr.node, crate::ast::Expr::Closure(_, _)) => {
                    if let Some((index, _)) = params
                        .iter()
                        .enumerate()
                        .find(|(_, param)| param.name() == Some(name.node.as_str()))
                    {
                        closure_params.insert(index, expr.span);
                    }
                }
                CallArg::Named(_, _) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {}
            }
        }
        closure_params
    }

    /// Bind a contextualized callback's owning type parameter to the concrete closure type inferred for the argument.
    pub(super) fn seed_contextual_callable_type_param_bindings(
        &self,
        params: &[CallableParam],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        let normal_params = params
            .iter()
            .filter(|param| param.kind == ParamKind::Normal)
            .collect::<Vec<_>>();
        let mut positional_index = 0usize;
        for (arg, arg_type) in args.iter().zip(arg_types) {
            let param = match arg {
                CallArg::Positional(_) => {
                    let param = normal_params.get(positional_index).copied();
                    positional_index += 1;
                    param
                }
                CallArg::Named(name, _) => normal_params
                    .iter()
                    .find(|param| param.name() == Some(name.node.as_str()))
                    .copied(),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => None,
            };
            let Some(param) = param else {
                continue;
            };
            if matches!(Self::call_arg_expr(arg).node, crate::ast::Expr::Closure(_, _))
                && let ResolvedType::TypeVar(name) | ResolvedType::Named(name) = &param.ty
            {
                bindings.insert(name.clone(), arg_type.clone());
            }
        }
    }

    /// Resolve one type parameter's canonical `CallableN[Args..., Return]` bound to a function signature.
    pub(super) fn source_callable_signature_for_param(
        &self,
        type_param: &str,
        bound_details: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) -> Option<ResolvedType> {
        let bound = bound_details
            .get(type_param)?
            .iter()
            .find(|bound| self.callable_trait_for_bound(bound).is_some())?;
        let callable = self.callable_trait_for_bound(bound)?;
        let arity = callables::info_for(callable).arity;
        if bound.type_args.len() != arity + 1 {
            return None;
        }
        let resolved = bound
            .type_args
            .iter()
            .map(|ty| substitute_resolved_type(ty, bindings))
            .collect::<Vec<_>>();
        let params = resolved[..arity]
            .iter()
            .enumerate()
            .map(|(index, ty)| CallableParam::named(format!("arg{index}"), ty.clone(), ParamKind::Normal))
            .collect();
        Some(ResolvedType::Function(params, Box::new(resolved[arity].clone())))
    }

    /// Return the concrete callable signature arguments represented by one function or nominal callable object.
    pub(super) fn source_callable_signature_args(
        &self,
        actual: &ResolvedType,
        bound: &crate::symbols::TypeBoundInfo,
    ) -> Option<Vec<ResolvedType>> {
        let callable = self.callable_trait_for_bound(bound)?;
        let arity = callables::info_for(callable).arity;
        let args = match actual {
            ResolvedType::Function(params, return_type) if params.len() == arity => params
                .iter()
                .map(|param| param.ty.clone())
                .chain(std::iter::once(return_type.as_ref().clone()))
                .collect(),
            ResolvedType::Named(type_name) => self.instantiated_trait_args_for_type(type_name, &[], &bound.name)?,
            ResolvedType::Generic(type_name, type_args) => {
                self.instantiated_trait_args_for_type(type_name, type_args, &bound.name)?
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                return self.source_callable_signature_args(inner, bound);
            }
            _ => return None,
        };
        (args.len() == arity + 1).then_some(args)
    }

    /// Close dependent binders from checked callable signatures, iterable elements and nominal trait adoptions.
    /// A bound can depend on a binder another bound just closed; the finite pass count follows that dependency set.
    pub(in crate::typechecker) fn infer_type_param_bindings_from_source_bounds(
        &self,
        bound_details: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        for _ in 0..=bound_details.len() {
            let previous = bindings.clone();
            for (type_param, bounds) in bound_details {
                let Some(actual) = bindings.get(type_param).cloned() else {
                    continue;
                };
                for bound in bounds {
                    let Some(arguments) = self.source_bound_arguments(&actual, bound) else {
                        continue;
                    };
                    if bound.type_args.len() != arguments.len() {
                        continue;
                    }
                    for (expected, actual) in bound.type_args.iter().zip(&arguments) {
                        let expected = substitute_resolved_type(expected, bindings);
                        self.infer_type_param_bindings(&expected, actual, bindings);
                    }
                }
            }
            if *bindings == previous {
                break;
            }
        }
    }

    /// Read a bound's actual arguments from its existing semantic relation, preserving standard protocol identity.
    pub(super) fn source_bound_arguments(
        &self,
        actual: &ResolvedType,
        bound: &crate::symbols::TypeBoundInfo,
    ) -> Option<Vec<ResolvedType>> {
        if self.callable_trait_for_bound(bound).is_some() {
            return self.source_callable_signature_args(actual, bound);
        }
        if self.bound_is_builtin_trait(bound, TraitId::Iterable) {
            return self.iterable_protocol_element_type(actual).map(|element| vec![element]);
        }
        match actual {
            ResolvedType::Named(name) => self.instantiated_trait_args_for_type(name, &[], &bound.name),
            ResolvedType::Generic(name, arguments) => {
                self.instantiated_trait_args_for_type(name, arguments, &bound.name)
            }
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => self.source_bound_arguments(inner, bound),
            _ => None,
        }
    }

    /// Infer concrete type bindings for generic type parameters from a parameter/argument type pair.
    ///
    /// This walks matching container structure recursively so constructor field checks and function calls can recover
    /// bindings such as `T -> String` from shapes like `Boxed[T]` versus `Boxed[String]`. Public-package aliases,
    /// provider-qualified spellings and the declaration carrier an imported module's signature names (`Box[T]` from
    /// `helpers`, against the caller's `Box[int]`) share that structure even when their names differ.
    /// Native re-exports compare by their shared identity; a source collection expands only when the native template
    /// needs its metadata-defaulted owner slots. Whole type parameters still bind the source argument unchanged.
    pub(in crate::typechecker) fn infer_type_param_bindings(
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
                    && (self.source_type_names_same_declaration(name, actual_name)
                        || (name.contains("::")
                            && actual_name.contains("::")
                            && self.rust_native_type_identity_path(name)
                                == self.rust_native_type_identity_path(actual_name))
                        || self
                            .module_qualified_nominals_compatible(
                                &ResolvedType::Named(name.clone()),
                                &ResolvedType::Named(actual_name.clone()),
                            )
                            .unwrap_or(false))
                {
                    for (e, a) in expected_args.iter().zip(actual_args.iter()) {
                        self.infer_type_param_bindings(e, a, bindings);
                    }
                } else if let Some(canonical) = self.rust_template_canonical_path(name)
                    && let Some(expanded) = self.rust_inference_owner_for_template(&canonical, actual)
                {
                    let template = ResolvedType::Generic(canonical, expected_args.clone());
                    self.infer_type_param_bindings(&template, &expanded, bindings);
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
            ResolvedType::TypeToken(expected_inner) => {
                if let ResolvedType::TypeToken(actual_inner) = actual {
                    self.infer_type_param_bindings(expected_inner, actual_inner, bindings);
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

    /// Render explicit call-site type arguments as the call spelled them, for arity diagnostics.
    pub(in crate::typechecker::check_expr) fn written_type_args(explicit_type_args: &[Spanned<Type>]) -> Vec<String> {
        explicit_type_args
            .iter()
            .map(|type_arg| type_arg.node.to_string())
            .collect()
    }

    /// Classify where the bindings a bound check sees came from: an explicit bracket list or inference.
    pub(in crate::typechecker::check_expr) fn type_argument_origin(
        explicit_type_args: &[Spanned<Type>],
    ) -> TypeArgumentOrigin {
        if explicit_type_args.is_empty() {
            TypeArgumentOrigin::Inferred
        } else {
            TypeArgumentOrigin::Explicit
        }
    }

    /// Return the display rule's refusal for a type argument that fails a `Display` bound, or `None` for any other
    /// bound or for a type argument the ordinary bound refusal describes (#1748).
    pub(super) fn unsatisfied_display_bound(
        &self,
        func_name: &str,
        type_param: &str,
        bound: &str,
        actual_ty: &ResolvedType,
        call_span: Span,
    ) -> Option<CompileError> {
        if builtin_traits::from_str(bound) != Some(TraitId::Display) {
            return None;
        }
        self.display_bound_refusal(func_name, type_param, actual_ty, call_span)
    }

    /// Emit diagnostics when inferred concrete generic bindings violate explicit `with` bounds.
    ///
    /// An `Eq` or `Hash` bound the provider inferred from its body (a compiled library's hashed type parameter, #1758)
    /// is refused with `INCAN-T0114`: a concrete type argument only when it is known to lack the derive, as for a
    /// callee of this checker's own modules, and the caller's own type parameter unless its declaration carries the
    /// bound.
    pub(in crate::typechecker::check_expr) fn emit_explicit_bound_errors(
        &mut self,
        func_name: &str,
        bounds_by_param: &std::collections::HashMap<String, Vec<String>>,
        bound_details_by_param: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        actual_origin: TypeArgumentOrigin,
        call_span: Span,
    ) {
        for (type_param, bounds) in bounds_by_param {
            let Some(actual_ty) = bindings.get(type_param) else {
                continue;
            };
            if let Some(details) = bound_details_by_param.get(type_param)
                && !details.is_empty()
            {
                // Inferred `Eq` and `Hash` bounds are the callee's hashed type parameter (#1758): a concrete type
                // argument is refused only when it is known to lack them, and the caller's own type parameter unless
                // its declaration carries them.
                let inferred_hash = details
                    .iter()
                    .filter(|bound| bound.inferred && is_hash_key_bound(bound))
                    .collect::<Vec<_>>();
                if !inferred_hash.is_empty() {
                    match self.active_type_param_name(actual_ty) {
                        Some(placeholder) => self.refuse_type_parameter_without_hash_bounds(
                            func_name,
                            type_param,
                            placeholder,
                            &inferred_hash,
                            bindings,
                            call_span,
                        ),
                        None => self.refuse_unhashable_type_argument(func_name, type_param, actual_ty, call_span),
                    }
                }
                if let Some(placeholder) = self.active_type_param_name(actual_ty) {
                    for bound in details
                        .iter()
                        .filter(|bound| bound.inferred && !is_hash_key_bound(bound))
                    {
                        if !self.active_type_param_satisfies_bound_info(placeholder, bound, bindings) {
                            self.errors.push(errors::generic_bound_not_satisfied(
                                func_name,
                                type_param,
                                &self.type_bound_display(bound, bindings),
                                self.generic_bound_target(&bound.name),
                                placeholder,
                                actual_origin,
                                call_span,
                            ));
                        }
                    }
                }
                // Every type argument is judged by the bounds its callee declares. An inferred bound is a Rust
                // requirement of the callee's generated body: the caller's own type parameter must carry it (above),
                // and rustc proves it for a concrete type argument.
                for bound in details.iter().filter(|bound| !bound.inferred) {
                    if !self.type_satisfies_explicit_bound_info(actual_ty, bound, bindings) {
                        let error = self
                            .unsatisfied_display_bound(func_name, type_param, &bound.name, actual_ty, call_span)
                            .unwrap_or_else(|| {
                                errors::generic_bound_not_satisfied(
                                    func_name,
                                    type_param,
                                    &self.type_bound_display(bound, bindings),
                                    self.generic_bound_target(&bound.name),
                                    &actual_ty.to_string(),
                                    actual_origin,
                                    call_span,
                                )
                            });
                        self.errors.push(error);
                    }
                }
                continue;
            }
            for bound in bounds {
                if !self.type_satisfies_explicit_bound(actual_ty, bound) {
                    let error = self
                        .unsatisfied_display_bound(func_name, type_param, bound, actual_ty, call_span)
                        .unwrap_or_else(|| {
                            errors::generic_bound_not_satisfied(
                                func_name,
                                type_param,
                                bound,
                                self.generic_bound_target(bound),
                                &actual_ty.to_string(),
                                actual_origin,
                                call_span,
                            )
                        });
                    self.errors.push(error);
                }
            }
        }
    }
}
