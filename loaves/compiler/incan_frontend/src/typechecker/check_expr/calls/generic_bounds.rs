//! Generic call-site inference, monomorph recording, and explicit bound validation.

use super::TypeChecker;
use crate::ast::{CallArg, DictEntry, Expr, ListEntry, Literal, ParamKind, Span, Spanned, Type, UnaryOp};
use crate::diagnostics::CompileError;
use crate::diagnostics::errors::{self, TypeArgumentOrigin};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::symbols::{CallableParam, FunctionInfo, MethodInfo, ResolvedType, TypeInfo};
use crate::typechecker::helpers::collection_type_id;
use incan_lang::interop::is_rust_capability_bound;
use incan_lang::lang::callables;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::traits::{self as builtin_traits, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::CanonicalSymbolId;

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
        if let Some(identity) = identity.clone() {
            self.type_info.record_resolved_identity(call_site_span, identity);
        }
        let first_error = self.errors.len();
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
    pub(in crate::typechecker::check_expr::calls) fn validate_function_call(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let errors_before_call = self.errors.len();
        let mut seeded_type_bindings: std::collections::HashMap<String, ResolvedType> =
            std::collections::HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != info.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    func_name,
                    &info.type_params,
                    &Self::written_type_args(explicit_type_args),
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
        self.infer_type_param_bindings_from_source_callables(&info.type_param_bound_details, &mut type_bindings);
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
        let callee_identity = self.called_function_identity(func_name);
        self.refuse_unhashable_type_arguments(
            func_name,
            callee_identity.as_ref(),
            &info.type_params,
            &instantiation,
            call_span,
        );
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
            self.record_call_site_monomorph_if_complete(call_span, &info.type_params, &type_bindings);
        }

        substitute_resolved_type(&info.return_type, &type_bindings)
    }

    /// Retype a generic call's literal arguments at its instantiation (see
    /// [`Self::retype_literal_arguments_at_instantiation`]) and close each binding the arguments left open with the
    /// type the literals give it, for a function and a generic method alike (#1561).
    fn close_bindings_from_literal_arguments(
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
    fn retype_literal_arguments_at_instantiation(
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
    fn literal_argument_spells_a_type_param(
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
    fn settle_constructor_argument_sides(
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
    fn arguments_with_parameter_types<'a, 'p>(
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
    fn close_instantiation_from_bindings(
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
    fn argument_type_param_bindings<'a>(
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
    fn names_type_the_checker_does_not_know(&self, ty: &ResolvedType) -> bool {
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
            if candidates.len() < 2 {
                continue;
            }
            let current_unifies = bindings.get(type_param).is_some_and(|current| {
                !is_open_binding(current, type_params)
                    && candidates
                        .iter()
                        .all(|candidate| self.types_compatible(candidate, current))
            });
            if current_unifies {
                continue;
            }
            if let Some(unifier) = candidates.iter().find(|unifier| {
                candidates
                    .iter()
                    .all(|candidate| self.types_compatible(candidate, unifier))
            }) {
                bindings.insert(type_param.clone(), unifier.clone());
            }
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

    /// Refuse a call that leaves a type parameter with a declared bound open: no argument fixes it, since every
    /// argument passed for a parameter that names it is built from literals that do not say what it is (`None`) or
    /// there is none, and the expected type does not name it either (#1561).
    ///
    /// The bound says what the callee needs of the type argument, and such a call names no type argument for the bound
    /// to hold of: `show(None)` for `def show[T with Display](o: Option[T])` and `make()` for
    /// `def make[T with Display]() -> list[T]` are refused by rustc for want of a type. Only a bound the checker judges
    /// counts: a trait whose declaration it has. A Rust capability marker (`Send`) or a Rust trait is left to the
    /// Rust compiler, which may fix the parameter from what the checker does not model (a future's output). An
    /// argument that fixes the parameter, a constructor argument whose open side the call settled, and explicit type
    /// arguments, which the explicit-argument path checks, leave nothing open.
    fn refuse_bounded_type_params_left_open(
        &mut self,
        callee: &str,
        type_params: &[String],
        bound_details: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        arguments: &[(&Spanned<Expr>, &ResolvedType)],
        bindings: &std::collections::HashMap<String, ResolvedType>,
        call_span: Span,
    ) {
        for type_param in type_params {
            let Some(bound) = bound_details.get(type_param).and_then(|bounds| {
                bounds.iter().find(|bound| {
                    !bound.inferred
                        && !is_rust_capability_bound(Self::type_bound_source_name(bound))
                        && self.lookup_trait_adoption_info(bound).is_some()
                })
            }) else {
                continue;
            };
            let open = bindings
                .get(type_param)
                .is_none_or(|ty| is_open_binding(ty, type_params));
            let only_literals_name_it = arguments.iter().all(|(expr, param_ty)| {
                first_open_type_param(param_ty, std::slice::from_ref(type_param)).is_none()
                    || is_built_from_literals(&expr.node)
            });
            if open && only_literals_name_it {
                self.errors.push(errors::bounded_type_param_left_open(
                    callee,
                    type_param,
                    &self.type_bound_display(bound, bindings),
                    call_span,
                ));
            }
        }
    }

    /// Side `side` (0 for success, 1 for error) of `ty` when it is a `Result`.
    fn result_side_of(ty: &ResolvedType, side: usize) -> Option<&ResolvedType> {
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
    pub(in crate::typechecker::check_expr) fn substitute_callable_params(
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
        let mut type_bindings = self.receiver_type_param_bindings(receiver_ty);
        let explicit_arity_ok =
            explicit_type_args.is_empty() || explicit_type_args.len() == method_info.type_params.len();

        // ---- RFC 054: explicit bracketed type arguments (partial map; `_` → CallSiteInfer omitted) ----
        if !explicit_type_args.is_empty() {
            if !explicit_arity_ok {
                self.errors.push(errors::explicit_type_arg_arity(
                    method,
                    &method_info.type_params,
                    &Self::written_type_args(explicit_type_args),
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
        self.infer_type_param_bindings_from_source_callables(&method_info.type_param_bound_details, &mut type_bindings);
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
            self.refuse_bounded_type_params_left_open(
                method,
                &method_info.type_params,
                &method_info.type_param_bound_details,
                &arguments,
                &instantiation,
                call_site_span,
            );
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

    /// Project a generic `CallableN` bound into the contextual function type used to check a callback argument.
    fn contextualize_source_callable_params(
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
                let ty = if let Some(span) = closure_params.get(&index) {
                    if let Some(signature) = generic_name
                        .and_then(|name| self.source_callable_signature_for_param(name, bound_details, bindings))
                    {
                        self.type_info.record_source_callable_closure(*span);
                        signature
                    } else {
                        param.ty.clone()
                    }
                } else {
                    param.ty.clone()
                };
                CallableParam {
                    name: param.name.clone(),
                    ty,
                    kind: param.kind,
                    has_default: param.has_default,
                    is_partial_preset: param.is_partial_preset,
                    is_mut: param.is_mut,
                }
            })
            .collect()
    }

    /// Map direct closure arguments to their bound normal parameter indexes.
    fn closure_argument_param_indexes(
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
    fn seed_contextual_callable_type_param_bindings(
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
    fn source_callable_signature_for_param(
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
    fn source_callable_signature_args(
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

    /// Infer still-open generic arguments from a function value or nominal callable object and its `CallableN` bound.
    fn infer_type_param_bindings_from_source_callables(
        &self,
        bound_details: &std::collections::HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
        bindings: &mut std::collections::HashMap<String, ResolvedType>,
    ) {
        for (type_param, bounds) in bound_details {
            let Some(actual) = bindings.get(type_param).cloned() else {
                continue;
            };
            let Some(bound) = bounds
                .iter()
                .find(|bound| self.callable_trait_for_bound(bound).is_some())
            else {
                continue;
            };
            let Some(callable_args) = self.source_callable_signature_args(&actual, bound) else {
                continue;
            };
            if bound.type_args.len() != callable_args.len() {
                continue;
            }
            for (expected, actual) in bound.type_args.iter().zip(callable_args.iter()) {
                let expected = substitute_resolved_type(expected, bindings);
                self.infer_type_param_bindings(&expected, actual, bindings);
            }
        }
    }

    /// Infer concrete type bindings for generic type parameters from a parameter/argument type pair.
    ///
    /// This walks matching container structure recursively so constructor field checks and function calls can recover
    /// bindings such as `T -> String` from shapes like `Boxed[T]` versus `Boxed[String]`. Public-package aliases and
    /// provider-qualified spellings of the same generic declaration share that structure even when their names differ.
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
                    && (name == actual_name
                        || self
                            .public_library_type_identities
                            .get(name)
                            .zip(self.public_library_type_identities.get(actual_name))
                            .is_some_and(|(expected_identity, actual_identity)| expected_identity == actual_identity)
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
    fn unsatisfied_display_bound(
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

/// Whether an inferred bound is the builtin `Eq` or `Hash` a hashed type parameter needs (#1758).
fn is_hash_key_bound(bound: &crate::symbols::TypeBoundInfo) -> bool {
    matches!(
        bound.name.rsplit("::").next().and_then(derives::from_str),
        Some(DeriveId::Eq | DeriveId::Hash)
    )
}

/// Return whether a type-parameter binding leaves the parameter open: unknown, or naming a type parameter still to be
/// inferred (the `T` a `[]` checked against `list[T]` binds `T` to).
fn is_open_binding(ty: &ResolvedType, type_params: &[String]) -> bool {
    matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
        || first_open_type_param(ty, type_params).is_some()
}

/// Return the first of `type_params` that `ty` names, at any depth, as a type variable still to be inferred.
///
/// A callee's parameter types name its type parameters as type variables; a type parameter of the caller's own body is
/// a named type there, and is not open.
pub(in crate::typechecker::check_expr) fn first_open_type_param(
    ty: &ResolvedType,
    type_params: &[String],
) -> Option<String> {
    match ty {
        ResolvedType::TypeVar(name) => type_params.contains(name).then(|| name.clone()),
        ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
            args.iter().find_map(|arg| first_open_type_param(arg, type_params))
        }
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner) => first_open_type_param(inner, type_params),
        ResolvedType::FrozenDict(key, value) => {
            first_open_type_param(key, type_params).or_else(|| first_open_type_param(value, type_params))
        }
        ResolvedType::Function(params, ret) => params
            .iter()
            .find_map(|param| first_open_type_param(&param.ty, type_params))
            .or_else(|| first_open_type_param(ret, type_params)),
        _ => None,
    }
}

/// Return whether `expr` is an integer literal or a negated one, looking through parentheses.
fn is_integer_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_integer_literal(&inner.node),
        Expr::Literal(Literal::Int(_)) => true,
        Expr::Unary(UnaryOp::Neg, operand) => matches!(operand.node, Expr::Literal(Literal::Int(_))),
        _ => false,
    }
}

/// Return whether `expr` is the `None` literal, looking through parentheses.
fn is_none_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_none_literal(&inner.node),
        Expr::Literal(Literal::None) => true,
        _ => false,
    }
}

/// Return whether `expr` is a list, dict, set or tuple literal, looking through parentheses.
fn is_collection_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Paren(inner) => is_collection_literal(&inner.node),
        Expr::List(_) | Expr::Dict(_) | Expr::Set(_) | Expr::Tuple(_) => true,
        _ => false,
    }
}

/// Return whether `expr` is built from literals alone: a literal (`None` included), a negated number literal, or a
/// list, dict, set or tuple literal whose entries are built from literals alone, with no spread.
///
/// Checking such an expression a second time records its types again and does nothing else.
fn is_built_from_literals(expr: &Expr) -> bool {
    match expr {
        Expr::Literal(_) => true,
        Expr::Paren(inner) => is_built_from_literals(&inner.node),
        Expr::Unary(UnaryOp::Neg, operand) => {
            matches!(operand.node, Expr::Literal(Literal::Int(_) | Literal::Float(_)))
        }
        Expr::List(entries) => entries.iter().all(|entry| match entry {
            ListEntry::Element(item) => is_built_from_literals(&item.node),
            ListEntry::Spread(_) => false,
        }),
        Expr::Dict(entries) => entries.iter().all(|entry| match entry {
            DictEntry::Pair(key, value) => is_built_from_literals(&key.node) && is_built_from_literals(&value.node),
            DictEntry::Spread(_) => false,
        }),
        Expr::Set(items) | Expr::Tuple(items) => items.iter().all(|item| is_built_from_literals(&item.node)),
        _ => false,
    }
}
