//! Literal argument retyping, argument type bindings and conflicting type arguments.

use super::*;

impl TypeChecker {
    /// Recheck an open native call argument after this source call has closed its parameter type.
    ///
    /// Native receiver inference can initially retain a source view containing the callee's own type parameter. Once
    /// another checked native carrier closes that parameter, the instantiated destination selects the concrete native
    /// owner and signature, just as an explicit annotation at the argument would. Already closed and nonnative
    /// arguments keep their first check.
    pub(super) fn settle_native_call_arguments_at_instantiation(
        &mut self,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        for (expression, parameter) in Self::arguments_with_parameters(params, args) {
            let Some(parameter) = parameter else { continue };
            if first_open_type_param(&parameter.ty, type_params).is_none()
                || !self.type_info.rust.checked_calls.contains_key(&expression.span)
            {
                continue;
            }
            let instantiated = substitute_resolved_type(&parameter.ty, bindings);
            if first_open_type_param(&instantiated, type_params).is_none() {
                self.check_expr_with_expected(expression, Some(&instantiated));
            }
        }
    }

    /// Retype a generic call's literal arguments at its instantiation (see
    /// [`Self::retype_literal_arguments_at_instantiation`]) and close each binding the arguments left open with the
    /// type the literals give it, for a function and a generic method alike (#1561).
    pub(super) fn close_bindings_from_literal_arguments(
        &mut self,
        callee: &str,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        instantiation: &std::collections::HashMap<String, ResolvedType>,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        let closed = self.retype_literal_arguments_at_instantiation(callee, type_params, params, args, instantiation);
        for (type_param, ty) in closed {
            if bindings
                .get(&type_param)
                .is_none_or(|bound| is_open_binding(bound, type_params))
            {
                bindings.insert(type_param, ty);
            }
        }
    }

    /// Give each literal argument of a generic call the type its parameter has in the instantiation the call made, or
    /// refuse a collection literal that leaves a type parameter nothing else fixes (#1859, #1862).
    ///
    /// A literal is checked against its parameter's declared type, which names the callee's type parameters: `1`
    /// against `T` stays an `int` although another argument binds `T` to `float` (`pick(2.5, 1)`), and `[]` against
    /// `list[T]` takes `list[T]`, a type the caller cannot name. Once the arguments are checked, the instantiation is
    /// the bindings that name no type parameter still to be inferred, completed from the arguments whose own types
    /// name none (the `5` of `first_or([], 5)` binds `T` although the `[]` bound it to itself first), where an integer
    /// literal binds a parameter only when no other argument does (`pick(1, 2.5)` binds `T` to `float`). An argument
    /// built from literals alone is then checked again against its instantiated parameter type, so the `1` takes
    /// `float` and the `[]` takes `list[int]`. A list, dict, set or tuple literal whose own type still names a type
    /// parameter the instantiation leaves open gives the call nothing to instantiate that parameter with, and is
    /// refused (`count([])`), unless a type parameter of that name is in scope at the call. Arguments that are not
    /// built from literals alone are not checked a second time. The instantiation is returned, so the call's
    /// parameter and result types take it where inference left a type parameter open.
    pub(super) fn retype_literal_arguments_at_instantiation(
        &mut self,
        callee: &str,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        instantiation: &std::collections::HashMap<String, ResolvedType>,
    ) -> std::collections::HashMap<String, ResolvedType> {
        let arguments = Self::arguments_with_parameters(params, args);
        let mut closed = instantiation
            .iter()
            .filter(|(_, ty)| !is_open_binding(ty, type_params))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        let (integer_literals, others): (Vec<_>, Vec<_>) =
            arguments.iter().partition(|(expr, _)| is_integer_literal(&expr.node));
        for (expr, param) in others.into_iter().chain(integer_literals) {
            let (Some(param), Some(arg_ty)) = (param, self.type_info.expr_type(expr.span)) else {
                continue;
            };
            if first_open_type_param(arg_ty, type_params).is_some() {
                continue;
            }
            let mut inferred = std::collections::HashMap::new();
            self.infer_type_param_bindings(&param.ty, arg_ty, &mut inferred);
            for (name, ty) in inferred {
                if !is_open_binding(&ty, type_params) {
                    closed.entry(name).or_insert(ty);
                }
            }
        }
        for (expr, param) in arguments {
            let Some(param) = param else {
                continue;
            };
            if first_open_type_param(&param.ty, type_params).is_none() || !is_built_from_literals(&expr.node) {
                continue;
            }
            let instantiated = substitute_resolved_type(&param.ty, &closed);
            if let Some(open) = first_open_type_param(&instantiated, type_params) {
                let literal_names_open_param = is_collection_literal(&expr.node)
                    && self
                        .type_info
                        .expr_type(expr.span)
                        .is_some_and(|recorded| first_open_type_param(recorded, type_params).is_some())
                    && !self.is_generic_placeholder_type(&ResolvedType::Named(open.clone()));
                if literal_names_open_param {
                    self.errors
                        .push(errors::generic_literal_argument_leaves_type_param_open(
                            callee, &open, expr.span,
                        ));
                }
                continue;
            }
            self.check_expr_with_expected(expr, Some(&instantiated));
        }
        closed
    }

    /// Return whether a call passes a collection or `None` literal, or an `Ok(...)` or `Err(...)` constructor (looking
    /// through parentheses), for a parameter whose declared type names one of the callee's type parameters: a value
    /// whose written form spells its type, which the declared parameter type would spell with the callee's own type
    /// parameter.
    ///
    /// A constructor spells the side of its `Result` it leaves open from its parameter's type, so `pick(Err(2), 5)` for
    /// `def pick[T](r: Result[T, int], d: T)` builds its `Err` with the `int` that `d` fixes `T` to, rather than with
    /// the callee's own `T`, which the caller cannot name (#1561).
    pub(super) fn literal_argument_spells_a_type_param(
        &self,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
    ) -> bool {
        Self::arguments_with_parameters(params, args)
            .into_iter()
            .any(|(expr, param)| {
                param.is_some_and(|param| first_open_type_param(&param.ty, type_params).is_some())
                    && (is_collection_literal(&expr.node)
                        || is_none_literal(&expr.node)
                        || self.returned_result_constructor(expr).is_some())
            })
    }

    /// Close the call's open type-parameter bindings from all of its arguments, then bind each one an `Ok(...)` or
    /// `Err(...)` argument's open side still leaves open to the type that side is built with when nothing fixes it
    /// (#1561).
    ///
    /// Such a constructor leaves its open side to the call (see the constructor check), and an argument whose type
    /// leaves a type parameter open binds it to nothing, so a later argument that fixes it wins: `pick(Err(2), 5)` for
    /// `def pick[T](r: Result[T, int], d: T)` takes `T` from `5`, whether `d` is a literal or not. A side no argument
    /// fixes is then the enclosing function's `Result` side, or `None`, as for a constructor bound to a local, and the
    /// call's parameter and result types say so.
    pub(super) fn settle_constructor_argument_sides(
        &self,
        type_params: &[String],
        params: &[CallableParam],
        args: &[CallArg],
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        let arguments = Self::arguments_with_parameters(params, args);
        if !arguments
            .iter()
            .any(|(expr, _)| self.returned_result_constructor(expr).is_some())
        {
            return;
        }
        let is_open = |bindings: &std::collections::HashMap<String, ResolvedType>, name: &str| {
            bindings
                .get(name)
                .is_none_or(|bound| is_open_binding(bound, type_params))
        };
        for (expr, param) in &arguments {
            let (Some(param), Some(arg_ty)) = (param, self.type_info.expr_type(expr.span)) else {
                continue;
            };
            let mut inferred = std::collections::HashMap::new();
            self.infer_type_param_bindings(&param.ty, arg_ty, &mut inferred);
            for (name, ty) in inferred {
                if !is_open_binding(&ty, type_params) && is_open(bindings, &name) {
                    bindings.insert(name, ty);
                }
            }
        }
        for (expr, param) in arguments {
            let (Some(param), Some(constructor)) = (param, self.returned_result_constructor(expr)) else {
                continue;
            };
            let open_side = usize::from(constructor == ConstructorId::Ok);
            let Some(ResolvedType::TypeVar(name)) = Self::result_side_of(&param.ty, open_side) else {
                continue;
            };
            if type_params.contains(name) && is_open(bindings, name) {
                bindings.insert(name.clone(), self.open_result_side_type(open_side));
            }
        }
    }

    /// Pair each argument with the type of the parameter it binds, as [`Self::arguments_with_parameters`] pairs them,
    /// leaving out an argument that binds no parameter.
    pub(in crate::typechecker::check_expr::calls) fn arguments_with_parameter_types<'a, 'p>(
        params: &'p [CallableParam],
        args: &'a [CallArg],
    ) -> Vec<(&'a Spanned<Expr>, &'p ResolvedType)> {
        Self::arguments_with_parameters(params, args)
            .into_iter()
            .filter_map(|(expr, param)| param.map(|param| (expr, &param.ty)))
            .collect()
    }

    /// Close each binding of `instantiation` that is still open with the call's own binding for that type parameter,
    /// once the call has settled the side an `Ok(...)` or `Err(...)` argument leaves open, so the bound checks hold
    /// the type the call is built with (#1561).
    pub(super) fn close_instantiation_from_bindings(
        instantiation: &mut std::collections::HashMap<String, ResolvedType>,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        type_params: &[String],
    ) {
        for (type_param, ty) in bindings {
            let open = instantiation
                .get(type_param)
                .is_none_or(|bound| is_open_binding(bound, type_params));
            if open && !is_open_binding(ty, type_params) {
                instantiation.insert(type_param.clone(), ty.clone());
            }
        }
    }

    /// Return, in argument order, the closed type each argument binds the callee's type parameter `type_param` to,
    /// read from the argument's recorded type against the type of the parameter it is passed to.
    ///
    /// An argument whose type leaves the parameter open (`[]` against `list[T]`, the side an `Err(...)` leaves open)
    /// binds nothing, and so does one whose binding names a type the checker does not know (an unresolved import, a
    /// Rust type), whose compatibility is left to the Rust compiler.
    pub(super) fn argument_type_param_bindings<'a>(
        &self,
        type_param: &str,
        type_params: &[String],
        arguments: &[(&'a Spanned<Expr>, &ResolvedType)],
    ) -> Vec<(&'a Spanned<Expr>, ResolvedType)> {
        arguments
            .iter()
            .filter_map(|(expr, param_ty)| {
                let arg_ty = self.type_info.expr_type(expr.span)?;
                let mut inferred = std::collections::HashMap::new();
                self.infer_type_param_bindings(param_ty, arg_ty, &mut inferred);
                let bound = inferred.remove(type_param)?;
                (!is_open_binding(&bound, type_params) && !self.names_type_the_checker_does_not_know(&bound))
                    .then_some((*expr, bound))
            })
            .collect()
    }

    /// Whether `ty` is unknown or names, at any depth, a type the checker has no declaration for: an unknown type, a
    /// Rust path, or a name that is neither a declared type, a trait nor a type parameter in scope.
    pub(super) fn names_type_the_checker_does_not_know(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Unknown | ResolvedType::RustPath(_) => true,
            ResolvedType::Named(name) => {
                !self.is_generic_placeholder_type(ty)
                    && self.lookup_semantic_type_info(name).is_none()
                    && self.lookup_semantic_trait_info(name).is_none()
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().any(|arg| self.names_type_the_checker_does_not_know(arg))
            }
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.names_type_the_checker_does_not_know(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.names_type_the_checker_does_not_know(key) || self.names_type_the_checker_does_not_know(value)
            }
            ResolvedType::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| self.names_type_the_checker_does_not_know(&param.ty))
                    || self.names_type_the_checker_does_not_know(ret)
            }
            _ => false,
        }
    }

    /// Bind each of the callee's type parameters that the arguments bind to different types to the one type every
    /// other binding widens to, when there is one (#1561).
    ///
    /// Argument checking keeps a parameter's first binding when a later argument's type is compatible with it, and
    /// leaves the parameter unknown when it is not, so `pick(small, wide)` for an `i8` and an `int` found no type for
    /// `T` while `pick(wide, small)` bound it to `int`. The call's binding is the argument binding that every other
    /// one is compatible with, following the numeric assignment rules of RFC 009, so both orders bind `T` to `int`.
    /// An argument built from literals alone takes its parameter's type in the call (`1` and `2.5` are `f32` beside an
    /// `f32` value), so it binds a parameter only through the literal retyping that follows. A binding every argument
    /// is compatible with is kept, and so is the binding of a parameter that fewer than two arguments bind, which
    /// argument checking settled; arguments that do not unify are left to [`Self::refuse_conflicting_type_arguments`].
    pub(in crate::typechecker::check_expr::calls) fn unify_argument_type_bindings(
        &self,
        type_params: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        for type_param in type_params {
            let candidates = self
                .argument_type_param_bindings(type_param, type_params, arguments)
                .into_iter()
                .filter(|(expr, _)| !is_built_from_literals(&expr.node))
                .map(|(_, ty)| ty)
                .collect::<Vec<_>>();
            self.unify_type_param_candidates(type_param, type_params, &candidates, bindings);
        }
    }

    /// Unify constructor field bindings, including one checked nested field that closes an open owner parameter.
    ///
    /// Ordinary generic calls retain [`Self::unify_argument_type_bindings`]'s multi-candidate rule. Constructor fields
    /// additionally authenticate the sole candidate through their recorded field binding, so a nested constructor can
    /// close the enclosing constructor's self-binding without changing general call inference.
    pub(in crate::typechecker::check_expr::calls) fn unify_constructor_argument_type_bindings(
        &self,
        type_params: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        for type_param in type_params {
            let candidates = self
                .argument_type_param_bindings(type_param, type_params, arguments)
                .into_iter()
                .filter(|(expression, _)| !is_built_from_literals(&expression.node))
                .collect::<Vec<_>>();
            if let [(expression, candidate)] = candidates.as_slice() {
                if self.type_info.constructor_field_binding(expression.span).is_some()
                    && bindings
                        .get(type_param)
                        .is_some_and(|current| is_open_binding(current, type_params))
                {
                    bindings.insert(type_param.clone(), candidate.clone());
                }
                continue;
            }
            let candidates = candidates.into_iter().map(|(_, ty)| ty).collect::<Vec<_>>();
            self.unify_type_param_candidates(type_param, type_params, &candidates, bindings);
        }
    }

    /// Bind `type_param` to the one candidate every other candidate is compatible with, when two or more arguments
    /// bind it and its current binding does not already unify them all.
    ///
    /// A parameter that fewer than two arguments bind keeps the binding argument checking settled. A candidate set
    /// with no unifier leaves the binding unchanged for [`Self::refuse_conflicting_type_arguments`] to refuse.
    fn unify_type_param_candidates(
        &self,
        type_param: &str,
        type_params: &[String],
        candidates: &[ResolvedType],
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        if candidates.len() < 2 {
            return;
        }
        let current_unifies = bindings.get(type_param).is_some_and(|current| {
            !is_open_binding(current, type_params)
                && candidates
                    .iter()
                    .all(|candidate| self.types_compatible(candidate, current))
        });
        if current_unifies {
            return;
        }
        if let Some(unifier) = candidates.iter().find(|unifier| {
            candidates
                .iter()
                .all(|candidate| self.types_compatible(candidate, unifier))
        }) {
            bindings.insert(type_param.to_owned(), unifier.clone());
        }
    }

    /// Refuse a call whose arguments bind one of the callee's type parameters to types that do not unify (#1561).
    ///
    /// `take(1, "s")` for `def take[T](x: T, y: T)` binds `T` to `str` from `"s"`, and the `1` is not a `str`; the
    /// generated call is refused by rustc. Each argument's binding must be compatible with the call's binding for the
    /// parameter, which [`Self::unify_argument_type_bindings`] and the literal retyping chose, so a narrower integer
    /// value widens to a wider one and an integer literal takes a float parameter's type (RFC 009). One argument is
    /// reported per type parameter, at the first that does not fit.
    pub(in crate::typechecker::check_expr::calls) fn refuse_conflicting_type_arguments(
        &mut self,
        callee: &str,
        type_params: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        for type_param in type_params {
            let argument_bindings = self.argument_type_param_bindings(type_param, type_params, arguments);
            let call_binding = bindings
                .get(type_param)
                .filter(|bound| !is_open_binding(bound, type_params))
                .or_else(|| argument_bindings.first().map(|(_, ty)| ty))
                .filter(|bound| !self.names_type_the_checker_does_not_know(bound));
            let Some(call_binding) = call_binding.cloned() else {
                continue;
            };
            if let Some((expr, found)) = argument_bindings
                .iter()
                .find(|(_, ty)| !self.types_compatible(ty, &call_binding))
            {
                self.errors.push(errors::conflicting_type_argument_bindings(
                    callee,
                    type_param,
                    &call_binding.to_string(),
                    &found.to_string(),
                    expr.span,
                ));
            }
        }
    }

    /// Record, for each argument whose parameter type names one of the callee's type parameters, the type that
    /// parameter has in the call as the argument's destination, when the argument's own type widens to it (#1561).
    ///
    /// Once the call binds `T` to `int` for an `i8` and an `int` argument, the `i8` value is written to an `int`
    /// place, so lowering widens it as it widens a value written to an `int` field (RFC 009); the generated call
    /// otherwise passes two Rust types for one type parameter. An argument built from literals alone was already
    /// checked against its instantiated parameter type.
    pub(in crate::typechecker::check_expr::calls) fn record_argument_destinations_at_bindings(
        &mut self,
        type_params: &[String],
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &std::collections::HashMap<String, ResolvedType>,
    ) {
        for (expr, param_ty) in arguments {
            if first_open_type_param(param_ty, type_params).is_none() || is_built_from_literals(&expr.node) {
                continue;
            }
            let destination = substitute_resolved_type(param_ty, bindings);
            if first_open_type_param(&destination, type_params).is_some() {
                continue;
            }
            let Some(arg_ty) = self.type_info.expr_type(expr.span).cloned() else {
                continue;
            };
            if arg_ty != destination {
                self.record_value_destination_if_compatible(expr.span, &arg_ty, &destination);
            }
        }
    }

    /// Side `side` (0 for success, 1 for error) of `ty` when it is a `Result`.
    pub(super) fn result_side_of(ty: &ResolvedType, side: usize) -> Option<&ResolvedType> {
        match ty {
            ResolvedType::Generic(name, sides)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && sides.len() == 2 =>
            {
                sides.get(side)
            }
            _ => None,
        }
    }

    /// Assert that call-site type parameters have been inferred.
    pub(super) fn assert_call_site_type_params_inferred(
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
}
