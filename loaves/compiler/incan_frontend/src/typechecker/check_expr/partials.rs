//! Partial expressions: presets, type bindings, and function, method and constructor targets.

use super::*;

impl TypeChecker {
    /// Type-check a local partial expression and return its projected callable type.
    ///
    /// A generic target (a generic function, or the constructor of a generic model, class or newtype) is instantiated
    /// first: the partial's written type arguments seed its type parameters, each preset value binds the ones its
    /// parameter's type names, and a type parameter left unbound is refused, since the partial is a value and a value
    /// is not generic. The instantiated target type is recorded at the target for lowering, and a generic function's
    /// type arguments at the partial for the call the partial makes.
    pub(super) fn check_partial_expr(&mut self, partial: &PartialExpr, span: Span) -> ResolvedType {
        let method_target = match self.partial_method_target_type(&partial.target) {
            Ok(method_target) => method_target,
            Err(()) => {
                for arg in &partial.args {
                    self.check_expr(&arg.value);
                }
                return ResolvedType::Unknown;
            }
        };
        let (target_ty, generic_target) = match method_target
            .map(|method_ty| (method_ty, None))
            .or_else(|| self.partial_constructor_target_type(&partial.target))
        {
            Some(target) => target,
            None => {
                let previous_span = self.generic_partial_target_span.replace(partial.target.span);
                let previous_target = self.generic_partial_target.take();
                let target_ty = self.check_expr(&partial.target);
                let generic_target = std::mem::replace(&mut self.generic_partial_target, previous_target);
                self.generic_partial_target_span = previous_span;
                (target_ty, generic_target)
            }
        };
        if generic_target.is_none() && !partial.type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let ResolvedType::Function(params, ret) = target_ty else {
            self.errors.push(CompileError::type_error(
                "Partial expression target must be a callable value".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        // Calling a stored callable is supported, but constructing another partial from one is not yet representable:
        // the existing callable signature does not retain enough provenance to distinguish a preset captured by the
        // target from one captured by this new partial. Reject this at the source boundary instead of accepting a
        // program that legacy lowering or Body IR must later refuse.
        if let Expr::Ident(name) = &partial.target.node
            && self
                .lookup_symbol(name)
                .is_some_and(|symbol| matches!(&symbol.kind, SymbolKind::Variable(_)))
        {
            self.errors.push(CompileError::type_error(
                "Partial application of a locally stored callable is not supported".to_string(),
                partial.target.span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        }

        let Some(projected) = self.project_partial_params("<local partial>", "<callable>", params, &partial.args, span)
        else {
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return ResolvedType::Unknown;
        };

        // ---- Seed a generic target's type parameters from the written type arguments ----
        let type_params = generic_target
            .as_ref()
            .map(|target| target.type_params.clone())
            .unwrap_or_default();
        let Some(mut bindings) = self.seed_partial_type_bindings(partial, generic_target.as_ref(), span) else {
            return ResolvedType::Unknown;
        };

        // ---- Check each preset against its parameter, binding the type parameters it names ----
        // A written type argument is not revised by a preset: a preset of another type is a mismatch against it.
        let param_named = |name: &str| projected.iter().find(|param| param.name() == Some(name));
        let mut preset_types = Vec::with_capacity(partial.args.len());
        let mut inferred = HashMap::new();
        for arg in &partial.args {
            let expected = param_named(arg.name.as_str())
                .map(|param| substitute_resolved_type(&param.ty, &bindings))
                .filter(|expected| calls::first_open_type_param(expected, &type_params).is_none());
            let actual = self.check_expr_with_expected(&arg.value, expected.as_ref());
            if let Some(param) = param_named(arg.name.as_str()) {
                self.infer_type_param_bindings(&param.ty, &actual, &mut inferred);
            }
            preset_types.push(actual);
        }
        for (type_param, inferred_ty) in inferred {
            bindings.entry(type_param).or_insert(inferred_ty);
        }

        // ---- Instantiate a generic target ----
        let (projected, ret) = match &generic_target {
            Some(target) => {
                let Some(instantiated) =
                    self.instantiate_partial_target(partial, target, projected, &ret, &bindings, span)
                else {
                    return ResolvedType::Unknown;
                };
                instantiated
            }
            None => (projected, *ret),
        };

        self.validate_partial_presets(partial, &projected, &preset_types);

        // A local partial retains its complete callable signature. Presets become defaulted, name-overrideable
        // slots; `is_partial_preset` preserves the separate positional rule that starts at the residual arguments.
        ResolvedType::Function(Self::local_partial_params(projected, &partial.args), Box::new(ret))
    }

    /// Validate previously checked preset values against the final projected parameter types. This runs after
    /// generic instantiation so a written binder remains authoritative over a conflicting inferred preset.
    pub(super) fn validate_partial_presets(
        &mut self,
        partial: &PartialExpr,
        projected: &[CallableParam],
        preset_types: &[ResolvedType],
    ) {
        for (arg, actual) in partial.args.iter().zip(preset_types) {
            if let Some(param) = projected.iter().find(|param| param.name() == Some(arg.name.as_str()))
                && !self.types_compatible(actual, &param.ty)
            {
                self.errors.push(errors::type_mismatch(
                    &param.ty.to_string(),
                    &actual.to_string(),
                    arg.value.span,
                ));
            }
        }
    }

    /// Seed a generic partial from its written arguments before preset inference. Arity errors still check every
    /// preset for independent diagnostics and stop instantiation; non-generic or inferred targets start empty.
    pub(super) fn seed_partial_type_bindings(
        &mut self,
        partial: &PartialExpr,
        target: Option<&GenericPartialTarget>,
        span: Span,
    ) -> Option<HashMap<String, ResolvedType>> {
        let Some(target) = target.filter(|_| !partial.type_args.is_empty()) else {
            return Some(HashMap::new());
        };
        if partial.type_args.len() != target.type_params.len() {
            self.errors.push(errors::explicit_type_arg_arity(
                &target.callee,
                &target.type_params,
                &Self::written_type_args(&partial.type_args),
                span,
            ));
            for arg in &partial.args {
                self.check_expr(&arg.value);
            }
            return None;
        }
        let written = partial
            .type_args
            .iter()
            .map(|type_arg| self.resolve_type_checked(type_arg))
            .collect::<Vec<_>>();
        Some(type_param_subst_map_call_site(&target.type_params, &written))
    }

    /// Close a generic partial after preset inference, retaining its target type and function monomorph facts for
    /// lowering. Every unfixed binder is diagnosed before bound checks or any target metadata is changed.
    pub(super) fn instantiate_partial_target(
        &mut self,
        partial: &PartialExpr,
        target: &GenericPartialTarget,
        projected: Vec<CallableParam>,
        ret: &ResolvedType,
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) -> Option<(Vec<CallableParam>, ResolvedType)> {
        let unfixed = target
            .type_params
            .iter()
            .filter(|type_param| {
                bindings.get(*type_param).is_none_or(|bound| {
                    matches!(bound, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
                        || calls::first_open_type_param(bound, &target.type_params).is_some()
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unfixed.is_empty() {
            for type_param in &unfixed {
                self.errors.push(errors::local_partial_type_param_unfixed(
                    &target.callee,
                    type_param,
                    span,
                ));
            }
            return None;
        }
        self.emit_explicit_bound_errors(
            &target.callee,
            &target.bounds,
            &target.bound_details,
            bindings,
            Self::type_argument_origin(&partial.type_args),
            span,
        );
        let instantiated_params = projected
            .iter()
            .map(|param| CallableParam {
                ty: substitute_resolved_type(&param.ty, bindings),
                ..param.clone()
            })
            .collect::<Vec<_>>();
        let instantiated_ret = substitute_resolved_type(ret, bindings);
        // Lowering reads the target's parameters from its recorded type and the function call's type arguments.
        if let Some(target_ty) = self.type_info.expr_type(partial.target.span).cloned() {
            self.record_expr_type(partial.target.span, substitute_resolved_type(&target_ty, bindings));
        }
        if self.generic_partial_target_names_a_function(&partial.target) {
            let type_args = target
                .type_params
                .iter()
                .filter_map(|type_param| bindings.get(type_param).cloned())
                .collect();
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert(span, type_args);
        }
        Some((instantiated_params, instantiated_ret))
    }

    /// Return the callable type of a local partial's target that is a method of a value (`partial user.label(...)`),
    /// with the receiver's type substituted for `Self` and for the type's own type parameters (RFC 084).
    ///
    /// The receiver is a place: a local, `self`, or a field of one. The partial evaluates it once, when it is built,
    /// and holds its own copy, as a closure holds a local it captures, so a method that changes its receiver (`mut
    /// self`) is refused: its change would reach only the copy. An overloaded method, which RFC 084 does not admit as a
    /// target, and a generic method, whose type arguments nothing could fix, are refused too. Each refusal is reported
    /// here and answers `Err`. Any other target answers `Ok(None)`, for the ordinary target rules. The method's
    /// identity and its resolved name are recorded at the target for lowering, which calls it on the held receiver.
    pub(super) fn partial_method_target_type(&mut self, target: &Spanned<Expr>) -> Result<Option<ResolvedType>, ()> {
        let Expr::Field(base, member) = &target.node else {
            return Ok(None);
        };
        if !self.partial_receiver_is_a_value_place(base) {
            return Ok(None);
        }
        let receiver_ty = self.check_expr(base);
        let (type_name, receiver_args) = match &receiver_ty {
            ResolvedType::Named(name) => (name.clone(), Vec::new()),
            ResolvedType::Generic(name, args) => (name.clone(), args.clone()),
            _ => return Ok(None),
        };
        let Some((type_params, method, info, overloaded)) =
            self.lookup_semantic_type_info(&type_name).and_then(|type_info| {
                let (type_params, aliases, methods, overloads) = match type_info {
                    TypeInfo::Model(model) => (
                        &model.type_params,
                        Some(&model.method_aliases),
                        &model.methods,
                        &model.method_overloads,
                    ),
                    TypeInfo::Class(class) => (
                        &class.type_params,
                        Some(&class.method_aliases),
                        &class.methods,
                        &class.method_overloads,
                    ),
                    TypeInfo::Newtype(newtype) => (
                        &newtype.type_params,
                        Some(&newtype.method_aliases),
                        &newtype.methods,
                        &newtype.method_overloads,
                    ),
                    TypeInfo::Enum(enum_info) => (
                        &enum_info.type_params,
                        None,
                        &enum_info.methods,
                        &enum_info.method_overloads,
                    ),
                    TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
                };
                let method = aliases
                    .and_then(|aliases| aliases.get(member))
                    .cloned()
                    .unwrap_or_else(|| member.clone());
                let info = methods.get(&method)?.clone();
                let overloaded = overloads.get(&method).is_some_and(|overloads| overloads.len() > 1);
                Some((type_params.clone(), method, info, overloaded))
            })
        else {
            return Ok(None);
        };
        self.refuse_inaccessible_method(&method, &info, &receiver_ty, target.span);
        let refusal = if overloaded {
            Some("it is overloaded, and a partial presets one callable")
        } else if !info.type_params.is_empty() {
            Some("it is generic, and nothing the partial writes fixes its type parameters")
        } else if info.receiver == Some(Receiver::Mutable) {
            Some(
                "it takes 'mut self', and the partial holds its own copy of the receiver, so the change would not reach it",
            )
        } else {
            None
        };
        if let Some(reason) = refusal {
            self.errors
                .push(errors::local_partial_method_target_refused(member, reason, target.span));
            return Err(());
        }
        if info.receiver.is_none() {
            return Ok(None);
        }
        let bindings = type_param_subst_map_call_site(&type_params, &receiver_args);
        let params = info
            .params
            .iter()
            .map(|param| CallableParam {
                ty: substitute_resolved_type(
                    &self.substitute_self_in_resolved_type(param.ty.clone(), &receiver_ty),
                    &bindings,
                ),
                ..param.clone()
            })
            .collect::<Vec<_>>();
        let ret = substitute_resolved_type(
            &self.substitute_self_in_resolved_type(info.return_type.clone(), &receiver_ty),
            &bindings,
        );
        let method_ty = ResolvedType::Function(params, Box::new(ret));
        if let Some(identity) = info.identity {
            self.type_info.record_resolved_identity(target.span, identity);
        }
        self.type_info
            .expressions
            .local_partial_method_targets
            .insert(target.span, method);
        self.record_expr_type(target.span, method_ty.clone());
        Ok(Some(method_ty))
    }

    /// Return whether a local partial's target base names a value the partial can hold: a local, `self`, or a field
    /// of one, through parentheses. A module, a type name and any other expression are not.
    pub(super) fn partial_receiver_is_a_value_place(&self, base: &Spanned<Expr>) -> bool {
        match &base.node {
            Expr::SelfExpr => true,
            Expr::Ident(name) => self
                .lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Variable(_))),
            Expr::Field(inner, _) | Expr::Paren(inner) => self.partial_receiver_is_a_value_place(inner),
            _ => false,
        }
    }

    /// Return whether a span is the target of the local partial being checked, where a generic function may be named
    /// as a value for the partial to instantiate.
    pub(in crate::typechecker::check_expr) fn is_generic_partial_target_span(&self, span: Span) -> bool {
        self.generic_partial_target_span == Some(span)
    }

    /// Whether a local partial's target is a generic function, whose call takes the partial's type arguments, rather
    /// than a generic type's constructor, which is built from its fields.
    pub(super) fn generic_partial_target_names_a_function(&self, target: &Spanned<Expr>) -> bool {
        !matches!(
            self.type_info.expressions.ident_kinds.get(&target.span),
            Some(IdentKind::TypeName)
        )
    }

    /// Return the constructor of the model, class or newtype a local partial names as its target, as a callable type,
    /// with the type's own type parameters when it is generic.
    ///
    /// RFC 084 makes constructor presets first-class, so `partial Reader(layer=layer)` inside a function presets the
    /// constructor's parameters as the top-level form does. The callable type is recorded at the target for lowering,
    /// with the target marked as a type name. A generic type's constructor returns the type over its own type
    /// parameters, which the partial instantiates.
    pub(super) fn partial_constructor_target_type(
        &mut self,
        target: &Spanned<Expr>,
    ) -> Option<(ResolvedType, Option<GenericPartialTarget>)> {
        let Expr::Ident(name) = &target.node else {
            return None;
        };
        let symbol_id = self.symbols.lookup(name)?;
        let kind = self.symbols.get(symbol_id)?.kind.clone();
        if !matches!(
            kind,
            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
        ) {
            return None;
        }
        let (params, return_type, _, type_params, bounds, bound_details) =
            Self::partial_callable_signature_from_kind(std::slice::from_ref(name), kind)?;
        if let Some(identity) = self.symbols.identity_of(symbol_id).cloned() {
            self.type_info.record_resolved_identity(target.span, identity);
        }
        self.type_info
            .expressions
            .ident_kinds
            .insert(target.span, IdentKind::TypeName);
        let (return_type, generic_target) = if type_params.is_empty() {
            (return_type, None)
        } else {
            let own_type = ResolvedType::Generic(
                name.clone(),
                type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
            );
            let generic_target = GenericPartialTarget {
                callee: name.clone(),
                type_params,
                bounds,
                bound_details,
            };
            (own_type, Some(generic_target))
        };
        let constructor_ty = ResolvedType::Function(params, Box::new(return_type));
        self.record_expr_type(target.span, constructor_ty.clone());
        Some((constructor_ty, generic_target))
    }
}
