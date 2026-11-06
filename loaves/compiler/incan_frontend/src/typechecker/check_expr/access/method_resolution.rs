//! Named, overloaded, source, generic-placeholder, trait-receiver and newtype method resolution.

use super::*;

impl TypeChecker {
    /// Resolve a method on a type's own methods or trait-adopted methods.
    ///
    /// Inherent methods and trait-provided methods both substitute `Self` in formal parameters and the return type
    /// using the call-site receiver so generic carriers typecheck consistently (#237).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::typechecker) fn resolve_named_method(
        &mut self,
        methods: &std::collections::HashMap<String, MethodInfo>,
        method_overloads: Option<&std::collections::HashMap<String, Vec<MethodInfo>>>,
        trait_adoptions: Option<&[TypeBoundInfo]>,
        method: &str,
        receiver_surface: MemberBindingSurface,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        if let Some(overloads) = method_overloads.and_then(|overloads| overloads.get(method)) {
            let trait_entries = trait_adoptions
                .map(|trait_adoptions| {
                    trait_adoptions
                        .iter()
                        .filter_map(|adoption| {
                            self.trait_method_entry_resolved_for_adoption(adoption, method, call_site_span)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let candidates = overloads
                .iter()
                .filter(|info| receiver_surface.accepts_method(info))
                .cloned()
                .map(|info| {
                    let dispatch = trait_entries
                        .iter()
                        .find(|entry| {
                            info.trait_target
                                .as_ref()
                                .is_none_or(|target| target.name == entry.origin_trait)
                                && self.method_sigs_compatible(&info, &entry.info)
                                && self.method_sigs_compatible(&entry.info, &info)
                        })
                        .map(|entry| {
                            self.resolved_trait_dispatch(
                                &entry.origin_trait,
                                entry.origin_type_args.clone(),
                                entry.origin_module_path.as_deref(),
                                entry.implementation_type_params.clone(),
                                entry.info.receiver == Some(Receiver::Mutable),
                            )
                        });
                    MethodCandidate { info, dispatch }
                })
                .collect::<Vec<_>>();
            if !candidates.is_empty() {
                return self.resolve_method_overload(
                    method,
                    &candidates,
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                    expected_return_ty,
                );
            }
        }
        if let Some(method_info) = methods
            .get(method)
            .filter(|method_info| receiver_surface.accepts_method(method_info))
        {
            return Some(self.check_resolved_generic_method_call(
                method,
                method_info.clone(),
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
                expected_return_ty,
            ));
        }
        if let Some(trait_adoptions) = trait_adoptions {
            let mut candidates = Vec::new();
            for adoption in trait_adoptions {
                if let Some(entry) = self.trait_method_entry_resolved_for_adoption(adoption, method, call_site_span) {
                    if !receiver_surface.accepts_method(&entry.info) {
                        continue;
                    }
                    let dispatch = self.resolved_trait_dispatch(
                        &entry.origin_trait,
                        entry.origin_type_args.clone(),
                        entry.origin_module_path.as_deref(),
                        entry.implementation_type_params.clone(),
                        entry.info.receiver == Some(Receiver::Mutable),
                    );
                    candidates.push(MethodCandidate {
                        info: entry.info,
                        dispatch: Some(dispatch),
                    });
                }
            }
            if !candidates.is_empty() {
                return self.resolve_method_overload(
                    method,
                    &candidates,
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                    expected_return_ty,
                );
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    /// Select a same-name method overload using call arguments and an optional expected return type.
    pub(super) fn resolve_method_overload(
        &mut self,
        method: &str,
        candidates: &[MethodCandidate],
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let mut viable: Vec<(usize, MethodCandidate)> = candidates
            .iter()
            .filter_map(|candidate| {
                self.method_candidate_match_score(&candidate.info, args, arg_types, receiver_ty, expected_return_ty)
                    .map(|score| (score, candidate.clone()))
            })
            .collect();

        if viable.is_empty() && expected_return_ty.is_some() {
            viable = candidates
                .iter()
                .filter_map(|candidate| {
                    self.method_candidate_match_score(&candidate.info, args, arg_types, receiver_ty, None)
                        .map(|score| (score, candidate.clone()))
                })
                .collect();
        }

        if viable.is_empty() {
            return candidates.first().map(|candidate| {
                if let Some(dispatch) = candidate.dispatch.clone() {
                    let dispatch = self.method_dispatch_substituting_call_site_self(dispatch, receiver_ty);
                    self.type_info
                        .record_resolved_method_call(call_site_span, method, dispatch);
                    let (params, _) = self.method_types_substituting_call_site_self(&candidate.info, receiver_ty);
                    self.type_info
                        .record_call_site_callable_params_for_dispatch(call_site_span, &params);
                }
                self.check_generic_method_call(
                    method,
                    candidate.info.clone(),
                    explicit_type_args,
                    args,
                    arg_types,
                    call_site_span,
                    receiver_ty,
                    expected_return_ty,
                )
            });
        }

        let best_score = viable.iter().map(|(score, _)| *score).max().unwrap_or(0);
        let mut best = viable
            .into_iter()
            .filter(|(score, _)| *score == best_score)
            .map(|(_, method_info)| method_info)
            .collect::<Vec<_>>();

        if best.len() == 1 {
            let candidate = best.remove(0);
            if let Some(dispatch) = candidate.dispatch.clone() {
                let dispatch = self.method_dispatch_substituting_call_site_self(dispatch, receiver_ty);
                self.type_info
                    .record_resolved_method_call(call_site_span, method, dispatch);
                let (params, _) = self.method_types_substituting_call_site_self(&candidate.info, receiver_ty);
                self.type_info
                    .record_call_site_callable_params_for_dispatch(call_site_span, &params);
            }
            return Some(self.check_resolved_generic_method_call(
                method,
                candidate.info,
                explicit_type_args,
                args,
                arg_types,
                call_site_span,
                receiver_ty,
                expected_return_ty,
            ));
        }

        self.errors
            .push(errors::ambiguous_trait_method_call(method, call_site_span));
        Some(ResolvedType::Unknown)
    }

    /// Authenticate a builtin receiver or the exact standard trait declaration its registered interface names. A
    /// same-spelled declaration from another module or package supplies no builtin authority.
    pub(super) fn is_builtin_receiver_owner(&self, name: &str) -> bool {
        let Some(identity) = self
            .symbols
            .lookup(name)
            .and_then(|symbol| self.symbols.identity_of(symbol))
        else {
            return false;
        };
        let SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return identity.origin == SymbolOrigin::Builtin;
        };
        let Some(trait_id) = core_traits::from_qualified_str(name) else {
            return false;
        };
        if identity.kind != SemanticSourceTargetKind::Trait
            || identity.declaration_name != core_traits::as_str(trait_id)
        {
            return false;
        }
        let Some(owner) = core_traits::source_module(trait_id) else {
            return false;
        };
        let source: Vec<String> = owner.split('.').map(str::to_owned).collect();
        let Some(declaring_module) = source.get(1..) else {
            return false;
        };
        module_path.as_slice() == declaring_module
            && self
                .loaves
                .owner_of(&source)
                .is_some_and(|entry| entry.artifact.manifest.name.as_str() == library.as_str())
    }

    /// Resolve a source-defined method when its owner has exactly one direct implementation for the requested name.
    /// An active Rust import never takes a source stub's method table; its native metadata remains authoritative.
    ///
    /// Ordinary method checking computes argument types before overload selection. That is useful for overloaded
    /// methods, but it is actively harmful for unambiguous source methods because collection literals can be checked
    /// before their parameter context is known. This path lets the declared method signature drive argument checking
    /// directly, the same way function calls do. A compiler-owned receiver keeps its registered builtin method
    /// contract; loading a standard trait's source declaration must not replace that contract with a source dispatch.
    pub(super) fn resolve_unambiguous_source_method_without_arg_prepass(
        &mut self,
        call: &SourceMethodPrepass<'_>,
    ) -> Option<ResolvedType> {
        let borrowed = match call.receiver_ty {
            ResolvedType::Ref(inner) => Some((inner.as_ref(), true)),
            ResolvedType::RefMut(inner) => Some((inner.as_ref(), false)),
            _ => None,
        };
        if let Some((receiver_ty, shared)) = borrowed {
            return self.resolve_unambiguous_source_method_without_arg_prepass(&SourceMethodPrepass {
                receiver_ty,
                shared_receiver: call.shared_receiver || shared,
                ..*call
            });
        }
        let type_name = match call.receiver_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return None,
        };
        if self
            .lookup_symbol(type_name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        {
            return None;
        }
        if Self::compiler_builtin_method_identity(call.receiver_ty, call.method).is_some()
            && self.is_builtin_receiver_owner(type_name)
        {
            return None;
        }
        let type_info = self.lookup_semantic_type_info(type_name).cloned();
        let Some(type_info) = type_info else {
            self.lookup_semantic_trait_info(type_name)?;
            let trait_args = match call.receiver_ty {
                ResolvedType::Generic(_, args) => args.clone(),
                _ => Vec::new(),
            };
            let adoption = TypeBoundInfo {
                name: type_name.clone(),
                source_name: self.trait_bound_source_name(type_name),
                type_args: trait_args,
                module_path: self.trait_bound_module_path(type_name),
                implementation_type_params: Vec::new(),
                inferred: false,
            };
            return self
                .resolve_unambiguous_adopted_trait_method_without_arg_prepass(std::slice::from_ref(&adoption), call);
        };
        match type_info {
            TypeInfo::Model(model) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives);
                self.resolve_source_owner_method_without_arg_prepass(
                    &model.methods,
                    &model.method_overloads,
                    &trait_adoptions,
                    call,
                )
            }
            TypeInfo::Class(class) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives);
                self.resolve_source_owner_method_without_arg_prepass(
                    &class.methods,
                    &class.method_overloads,
                    &trait_adoptions,
                    call,
                )
            }
            TypeInfo::Enum(en) => {
                let trait_adoptions = self.trait_adoptions_for_type_methods(&en.trait_adoptions, &en.derives);
                self.resolve_source_owner_method_without_arg_prepass(
                    &en.methods,
                    &en.method_overloads,
                    &trait_adoptions,
                    call,
                )
            }
            TypeInfo::Newtype(nt) => {
                let resolved_method = self.resolve_newtype_method_name(&nt, call.method);
                let resolved_call = SourceMethodPrepass {
                    method: resolved_method,
                    receiver_surface: call.receiver_surface,
                    type_args: call.type_args,
                    args: call.args,
                    span: call.span,
                    receiver_ty: call.receiver_ty,
                    expected_return_ty: call.expected_return_ty,
                    shared_receiver: call.shared_receiver,
                };
                let trait_adoptions = self.newtype_trait_adoptions_for_type_methods(type_name, &nt);
                let ret = self.resolve_source_owner_method_without_arg_prepass(
                    &nt.methods,
                    &nt.method_overloads,
                    &trait_adoptions,
                    &resolved_call,
                )?;
                if nt.is_rusttype {
                    self.maybe_record_rusttype_return_coercion(&nt, resolved_method, &ret, call.span);
                }
                Some(ret)
            }
            _ => None,
        }
    }

    /// Resolve a zero-argument `receiver.method()` the compiler synthesizes, exactly as the written call is resolved
    /// on a source-declared receiver.
    ///
    /// This is the first resolution a written call reaches (`check_method_call_with_expected`): the owner's sole
    /// direct method, else its sole adopted trait method, with the same recorded facts at `call_span`, and the same
    /// trait-module inheritance from `receiver_span`. `None` when that path does not decide the call, in which case a
    /// written call falls through to the general method resolution.
    pub(in crate::typechecker::check_expr) fn resolve_synthesized_source_method_call(
        &mut self,
        method: &str,
        receiver_ty: &ResolvedType,
        receiver_span: Span,
        call_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let call = SourceMethodPrepass {
            method,
            receiver_surface: MemberBindingSurface::Instance,
            type_args: &[],
            args: &[],
            span: call_span,
            receiver_ty,
            expected_return_ty,
            shared_receiver: false,
        };
        let ret = self.resolve_unambiguous_source_method_without_arg_prepass(&call)?;
        self.type_info
            .inherit_same_trait_method_module(receiver_span, call_span);
        Some(ret)
    }

    /// Resolve one source owner's sole direct or adopted method with declared argument context.
    pub(super) fn resolve_source_owner_method_without_arg_prepass(
        &mut self,
        methods: &std::collections::HashMap<String, MethodInfo>,
        method_overloads: &std::collections::HashMap<String, Vec<MethodInfo>>,
        trait_adoptions: &[TypeBoundInfo],
        call: &SourceMethodPrepass<'_>,
    ) -> Option<ResolvedType> {
        let method_info = if let Some(overloads) = method_overloads.get(call.method) {
            let mut matching = overloads
                .iter()
                .filter(|method| call.receiver_surface.accepts_method(method));
            let method = matching.next().cloned();
            if matching.next().is_some() {
                return None;
            }
            method
        } else {
            methods
                .get(call.method)
                .filter(|method| call.receiver_surface.accepts_method(method))
                .cloned()
        };
        let Some(method_info) = method_info else {
            return self.resolve_unambiguous_adopted_trait_method_without_arg_prepass(trait_adoptions, call);
        };
        if call.shared_receiver && method_info.receiver == Some(Receiver::Mutable) {
            return None;
        }

        Some(self.check_resolved_generic_method_call(
            call.method,
            method_info,
            call.type_args,
            call.args,
            &[],
            call.span,
            call.receiver_ty,
            call.expected_return_ty,
        ))
    }

    /// Resolve one adopted-trait method before checking callback-shaped arguments without their declared context.
    pub(super) fn resolve_unambiguous_adopted_trait_method_without_arg_prepass(
        &mut self,
        trait_adoptions: &[TypeBoundInfo],
        call: &SourceMethodPrepass<'_>,
    ) -> Option<ResolvedType> {
        let mut candidates = trait_adoptions
            .iter()
            .filter_map(|adoption| self.trait_method_entry_resolved_for_adoption(adoption, call.method, call.span))
            .filter(|candidate| call.receiver_surface.accepts_method(&candidate.info))
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return None;
        }
        let candidate = candidates.remove(0);
        if call.shared_receiver && candidate.info.receiver == Some(Receiver::Mutable) {
            return None;
        }
        let dispatch = self.resolved_trait_dispatch(
            &candidate.origin_trait,
            candidate.origin_type_args.clone(),
            candidate.origin_module_path.as_deref(),
            candidate.implementation_type_params.clone(),
            candidate.info.receiver == Some(Receiver::Mutable),
        );
        let dispatch = self.method_dispatch_substituting_call_site_self(dispatch, call.receiver_ty);
        self.type_info
            .record_resolved_method_call(call.span, call.method, dispatch);
        let (params, _) = self.method_types_substituting_call_site_self(&candidate.info, call.receiver_ty);
        self.type_info
            .record_call_site_callable_params_for_dispatch(call.span, &params);
        Some(self.check_resolved_generic_method_call(
            call.method,
            candidate.info,
            call.type_args,
            call.args,
            &[],
            call.span,
            call.receiver_ty,
            call.expected_return_ty,
        ))
    }

    /// Return a compatibility score for one method candidate.
    ///
    /// Compatibility admits useful coercions such as lossless numeric widening, but overload selection must prefer the
    /// candidate that most directly matches the call site. Exact argument and contextual return matches therefore score
    /// higher than merely compatible matches; ties remain ambiguous.
    pub(super) fn method_candidate_match_score(
        &self,
        candidate: &MethodInfo,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<usize> {
        let (mut params, mut return_type) = self.method_types_substituting_call_site_self(candidate, receiver_ty);
        if let Some(identity) = &candidate.identity {
            if let Some(values) = self
                .ownership_call_value_parameters(Some(identity), &params, Some(receiver_ty), identity.declaration_span)
                .ok()?
            {
                params = values;
            }
            if let Some(value) = self
                .ownership_call_value_result(Some(identity), &return_type, identity.declaration_span)
                .ok()?
            {
                return_type = value;
            }
        }
        let mut score = 0usize;
        if let Some(expected) = expected_return_ty
            && !self.types_compatible(&return_type, expected)
        {
            return None;
        } else if let Some(expected) = expected_return_ty {
            score += self.type_match_score(&return_type, expected);
        }
        let normal_params: Vec<&CallableParam> =
            params.iter().filter(|param| param.kind == ParamKind::Normal).collect();
        let rest_positional = params.iter().find(|param| param.kind == ParamKind::RestPositional);
        let rest_keyword = params.iter().find(|param| param.kind == ParamKind::RestKeyword);
        let mut normal_bound = vec![false; normal_params.len()];
        let mut positional_index = 0usize;
        let mut named_seen = std::collections::HashSet::new();
        for (arg, arg_ty) in args.iter().zip(arg_types.iter()) {
            match arg {
                CallArg::Positional(_) => {
                    if let Some(param) = normal_params.get(positional_index) {
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                        normal_bound[positional_index] = true;
                        positional_index += 1;
                    } else {
                        let param = rest_positional?;
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                    }
                }
                CallArg::Named(name, _) => {
                    if !named_seen.insert(name.node.as_str()) {
                        return None;
                    }
                    if let Some((normal_idx, param)) = normal_params
                        .iter()
                        .enumerate()
                        .find(|(_, param)| param.name() == Some(name.node.as_str()))
                    {
                        if normal_bound[normal_idx] {
                            return None;
                        }
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                        normal_bound[normal_idx] = true;
                    } else {
                        let param = rest_keyword?;
                        if !self.types_compatible(arg_ty, &param.ty) {
                            return None;
                        }
                        score += self.type_match_score(arg_ty, &param.ty);
                    }
                }
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => return Some(score),
            }
        }
        normal_params
            .iter()
            .zip(normal_bound.iter())
            .all(|(param, bound)| *bound || param.has_default)
            .then_some(score)
    }

    /// Score how directly `actual` matches `expected` for overload ranking.
    pub(super) fn type_match_score(&self, actual: &ResolvedType, expected: &ResolvedType) -> usize {
        match (actual, expected) {
            (ResolvedType::Unknown, _)
            | (_, ResolvedType::Unknown)
            | (ResolvedType::TypeVar(_), _)
            | (_, ResolvedType::TypeVar(_))
            | (ResolvedType::CallSiteInfer, _)
            | (_, ResolvedType::CallSiteInfer) => 0,
            (ResolvedType::Generic(actual_name, actual_args), ResolvedType::Generic(expected_name, expected_args))
                if actual_name == expected_name && actual_args.len() == expected_args.len() =>
            {
                4 + actual_args
                    .iter()
                    .zip(expected_args.iter())
                    .map(|(actual_arg, expected_arg)| self.type_match_score(actual_arg, expected_arg))
                    .sum::<usize>()
            }
            (ResolvedType::Tuple(actual_items), ResolvedType::Tuple(expected_items))
                if actual_items.len() == expected_items.len() =>
            {
                4 + actual_items
                    .iter()
                    .zip(expected_items.iter())
                    .map(|(actual_item, expected_item)| self.type_match_score(actual_item, expected_item))
                    .sum::<usize>()
            }
            (ResolvedType::FrozenList(actual_item), ResolvedType::FrozenList(expected_item)) => {
                4 + self.type_match_score(actual_item, expected_item)
            }
            (
                ResolvedType::FrozenDict(actual_key, actual_value),
                ResolvedType::FrozenDict(expected_key, expected_value),
            ) => {
                4 + self.type_match_score(actual_key, expected_key)
                    + self.type_match_score(actual_value, expected_value)
            }
            _ if actual == expected => 16,
            _ if self.rust_type_identities_compatible(actual, expected) == Some(true) => 12,
            _ => 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Resolve a method call on a generic placeholder using the active `T with Trait[...]` bound stack.
    /// A shared reference exposes the referent's shared methods and substitutes that referent for Self; it never
    /// authorizes a mutable receiver through the reference.
    pub(in crate::typechecker) fn resolve_generic_placeholder_method(
        &mut self,
        placeholder_name: &str,
        method: &str,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let active_bounds = self
            .innermost_type_param_bounds(placeholder_name)
            .map(<[TypeBoundInfo]>::to_vec)
            .unwrap_or_default();
        // A `Hash` bound is met only by `@derive(Hash)`, which gives a type no `__hash__` (#1822), so the method the
        // stdlib trait declares has nothing to call through the bound.
        if method == HASH_DUNDER
            && active_bounds
                .iter()
                .any(|bound| self.bound_is_builtin_trait(bound, TraitId::Hash))
        {
            self.errors
                .push(errors::hash_method_through_hash_bound(placeholder_name, call_site_span));
            return Some(ResolvedType::Unknown);
        }
        let mut candidates = Vec::new();
        for bound in &active_bounds {
            if let Some(entry) = self.trait_method_entry_resolved_for_adoption(bound, method, call_site_span) {
                if matches!(receiver_ty, ResolvedType::Ref(_)) && entry.info.receiver == Some(Receiver::Mutable) {
                    continue;
                }
                let dispatch = self.resolved_trait_dispatch(
                    &entry.origin_trait,
                    entry.origin_type_args.clone(),
                    entry.origin_module_path.as_deref(),
                    entry.implementation_type_params.clone(),
                    entry.info.receiver == Some(Receiver::Mutable),
                );
                candidates.push(MethodCandidate {
                    info: entry.info,
                    dispatch: Some(dispatch),
                });
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let receiver = match receiver_ty {
            ResolvedType::Ref(inner) => inner.as_ref(),
            _ => receiver_ty,
        };
        self.resolve_method_overload(
            method,
            &candidates,
            explicit_type_args,
            args,
            arg_types,
            call_site_span,
            receiver,
            expected_return_ty,
        )
    }

    /// Resolve a method call whose receiver is explicitly typed as a trait.
    ///
    /// Trait annotations expose only the annotated trait and its supertraits. They must not fall through to the
    /// permissive external-generic path, because that would let root-trait values call methods declared only on
    /// narrower subtraits.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::typechecker) fn resolve_trait_receiver_method(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        receiver_surface: MemberBindingSurface,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let (trait_name, trait_args) = match receiver_ty {
            ResolvedType::Named(name) => (name.as_str(), Vec::new()),
            ResolvedType::Generic(name, args) => (name.as_str(), args.clone()),
            _ => return None,
        };
        self.lookup_semantic_trait_info(trait_name)?;

        let adoption = TypeBoundInfo {
            name: trait_name.to_string(),
            source_name: self.trait_bound_source_name(trait_name),
            type_args: trait_args,
            module_path: self.trait_bound_module_path(trait_name),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.resolve_named_method(
            &std::collections::HashMap::new(),
            None,
            Some(std::slice::from_ref(&adoption)),
            method,
            receiver_surface,
            explicit_type_args,
            args,
            arg_types,
            call_site_span,
            receiver_ty,
            expected_return_ty,
        )
    }

    /// Resolve a method on a trait-typed value whose trait this module never bound, through the trait's owning module.
    ///
    /// A trait method may return a value of its own trait (`FallibleIterator.map` returns `FallibleIterator[U, E]`),
    /// and a consumer can hold such a value without importing the trait: `numbers().map(double)` on a dependency's
    /// adopter is checked through the adopter's own protocol, which the dependency's type carries together with the
    /// trait's owning module. The next call in the chain names the trait only by its spelling, which is not bound here,
    /// so it used to stay unresolved and the generated program called the method with the trait out of scope (#1761).
    ///
    /// The owning module is taken from what the checker already proved: the receiver's own trait dispatch when the
    /// receiver is a call on the same trait, otherwise the stdlib module that trait resolution loaded it from. The
    /// method then resolves exactly as it does on an adopter that carries the trait with that module, and records the
    /// same dispatch, so lowering names the trait by its path. A name that is bound here, as a trait or as a type, is
    /// left to ordinary resolution, and nothing is resolved when no owning module is known.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_unbound_trait_receiver_method(
        &mut self,
        receiver_span: Span,
        receiver_ty: &ResolvedType,
        method: &str,
        receiver_surface: MemberBindingSurface,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> Option<ResolvedType> {
        let (trait_name, trait_args) = match receiver_ty {
            ResolvedType::Named(name) => (name.as_str(), Vec::new()),
            ResolvedType::Generic(name, args) => (name.as_str(), args.clone()),
            _ => return None,
        };
        if self.lookup_semantic_trait_info(trait_name).is_some() || self.lookup_semantic_type_info(trait_name).is_some()
        {
            return None;
        }
        let receiver_module = match self.type_info.resolved_method_call(receiver_span) {
            Some(ResolvedMethodCall {
                dispatch:
                    crate::typechecker::ResolvedMethodDispatch::Trait {
                        trait_name: receiver_trait,
                        module_path: Some(module_path),
                        ..
                    },
                ..
            }) if receiver_trait == trait_name => Some(module_path.clone()),
            _ => None,
        };
        let module_path =
            receiver_module.or_else(|| incan_lang::lang::stdlib::trait_method_module_segments(trait_name))?;
        let adoption = TypeBoundInfo {
            name: trait_name.to_string(),
            source_name: Some(trait_name.to_string()),
            type_args: trait_args,
            module_path: Some(module_path),
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        self.resolve_named_method(
            &std::collections::HashMap::new(),
            None,
            Some(std::slice::from_ref(&adoption)),
            method,
            receiver_surface,
            explicit_type_args,
            args,
            arg_types,
            call_site_span,
            receiver_ty,
            expected_return_ty,
        )
    }

    /// Resolve a newtype/rusttype rebound method alias to its target method name.
    pub(in crate::typechecker) fn resolve_newtype_method_name<'a>(
        &self,
        newtype: &'a NewtypeInfo,
        method: &'a str,
    ) -> &'a str {
        newtype
            .method_rebindings
            .get(method)
            .map(String::as_str)
            .unwrap_or(method)
    }

    /// When a `rusttype` method resolves against an underlying Rust method with metadata, compare the Rust return type
    /// against the Incan-declared return type. If they differ in a coercible way (e.g. `&str` vs `String`), record a
    /// return coercion for `span` so lowering can wrap the call in `InteropCoerce`.
    ///
    /// This runs unconditionally but is a no-op when no Rust metadata covers the underlying method.
    pub(super) fn maybe_record_rusttype_return_coercion(
        &mut self,
        nt: &NewtypeInfo,
        method: &str,
        incan_ret: &ResolvedType,
        span: Span,
    ) {
        // Only relevant for rusttypes backed by a known Rust path.
        let ResolvedType::RustPath(underlying_path) = &nt.underlying else {
            return;
        };
        // Consult metadata for the actual Rust return type.
        let Some(sig) = self.rust_method_signature(underlying_path, method) else {
            return;
        };
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), incan_ret, span);
    }
}
