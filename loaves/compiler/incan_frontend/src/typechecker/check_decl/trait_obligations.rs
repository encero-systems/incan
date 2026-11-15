//! Trait abstract-member obligations, default overrides, and cross-trait method collisions.

use super::*;

impl TypeChecker {
    /// Group abstract (`...`) methods required by `trait_name` and its transitive supertraits by method name.
    ///
    /// Each group lists `(declaring_trait, signature)` after supertrait shadowing so diamonds can be merged or rejected
    /// consistently with [`Self::enforce_trait_abstract_methods`].
    pub(super) fn grouped_trait_abstract_method_obligations(
        &self,
        trait_name: &str,
        explicit_root: Option<(&TraitInfo, &[ResolvedType])>,
    ) -> HashMap<String, Vec<(String, MethodInfo)>> {
        let raw = if let Some((trait_info, trait_args)) = explicit_root {
            let mut entries = Vec::new();
            let mut seen = HashSet::new();
            self.collect_instantiated_trait_method_entries(
                trait_name,
                trait_info,
                trait_args,
                None,
                &mut seen,
                &mut entries,
            );
            entries
                .into_iter()
                .map(|entry| (entry.method_name, entry.origin_trait, entry.info))
                .collect()
        } else {
            let mut entries = Vec::new();
            if let Some(root) = self.lookup_semantic_trait_info(trait_name) {
                for (method, info) in &root.methods {
                    entries.push((method.clone(), trait_name.to_string(), info.clone()));
                }
            }
            for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
                let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                    continue;
                };
                let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
                for (method, info) in &supertrait_info.methods {
                    entries.push((
                        method.clone(),
                        supertrait_name.clone(),
                        substitute_method_info(info, &subst),
                    ));
                }
            }
            entries
        };
        let mut map: HashMap<String, Vec<(String, MethodInfo)>> = HashMap::new();
        for (method, origin, info) in raw {
            map.entry(method).or_default().push((origin, info));
        }
        let mut out = HashMap::new();
        for (m, entries) in map {
            let filtered = self
                .filter_supertrait_dominated_entries(entries)
                .into_iter()
                .filter(|(_, info)| !info.has_body)
                .collect::<Vec<_>>();
            if !filtered.is_empty() {
                out.insert(m, filtered);
            }
        }
        out
    }

    /// Group abstract (`...`) properties required by `trait_name` and its transitive supertraits.
    pub(super) fn grouped_trait_abstract_property_obligations(
        &self,
        trait_name: &str,
        explicit_root: Option<(&TraitInfo, &[ResolvedType])>,
    ) -> HashMap<String, Vec<(String, PropertyInfo)>> {
        let raw = if let Some((trait_info, trait_args)) = explicit_root {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            self.collect_instantiated_trait_abstract_property_entries(
                trait_name, trait_info, trait_args, &mut seen, &mut out,
            );
            out
        } else {
            let mut out = Vec::new();
            if let Some(root) = self.lookup_semantic_trait_info(trait_name) {
                for (p, info) in &root.properties {
                    if !info.has_body {
                        out.push((p.clone(), trait_name.to_string(), info.clone()));
                    }
                }
            }
            for (supertrait_name, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
                let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                    continue;
                };
                let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
                for (p, info) in &supertrait_info.properties {
                    if !info.has_body {
                        out.push((
                            p.clone(),
                            supertrait_name.clone(),
                            substitute_property_info(info, &subst),
                        ));
                    }
                }
            }
            out
        };

        let mut map: HashMap<String, Vec<(String, PropertyInfo)>> = HashMap::new();
        for (property, origin, info) in raw {
            map.entry(property).or_default().push((origin, info));
        }
        let mut out = HashMap::new();
        for (property, entries) in map {
            let filtered = self.filter_supertrait_dominated_property_entries(entries);
            if !filtered.is_empty() {
                out.insert(property, filtered);
            }
        }
        out
    }

    /// Check one concrete property implementation against one trait property requirement.
    pub(super) fn check_impl_against_trait_property_requirement(
        &mut self,
        type_name: &str,
        via_trait: &str,
        property_name: &str,
        property_info: &PropertyInfo,
        properties: &HashMap<String, PropertyInfo>,
        adoption_span: Span,
    ) {
        match properties.get(property_name) {
            None => self
                .errors
                .push(errors::missing_trait_property(via_trait, property_name, adoption_span)),
            Some(found) => {
                if !self.types_compatible(&found.return_type, &property_info.return_type) {
                    self.errors.push(errors::trait_property_signature_mismatch(
                        via_trait,
                        type_name,
                        property_name,
                        &property_info.return_type.to_string(),
                        &found.return_type.to_string(),
                        adoption_span,
                    ));
                }
            }
        }
    }

    /// Enforce abstract properties from `trait_name` and its supertraits on a concrete type.
    pub(super) fn enforce_trait_abstract_properties(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        properties: &HashMap<String, PropertyInfo>,
    ) {
        let grouped =
            self.grouped_trait_abstract_property_obligations(trait_name, trait_args.map(|args| (trait_info, args)));
        let mut property_names: Vec<String> = grouped.keys().cloned().collect();
        property_names.sort();
        for property_name in property_names {
            let Some(group) = grouped.get(&property_name) else {
                continue;
            };
            let exp0 = &group[0].1;
            if group.len() == 1 {
                self.check_impl_against_trait_property_requirement(
                    type_name,
                    &group[0].0,
                    property_name.as_str(),
                    exp0,
                    properties,
                    adoption_span,
                );
                continue;
            }
            let all_mutually_compat = group.iter().all(|(_, e)| {
                self.types_compatible(&exp0.return_type, &e.return_type)
                    && self.types_compatible(&e.return_type, &exp0.return_type)
            });
            if !all_mutually_compat {
                self.errors.push(errors::trait_property_conflict(
                    &group[0].0,
                    &group[1].0,
                    property_name.as_str(),
                    adoption_span,
                ));
                continue;
            }
            let all_identical = group.iter().all(|(_, e)| property_infos_identical(exp0, e));
            if all_identical {
                self.check_impl_against_trait_property_requirement(
                    type_name,
                    &group[0].0,
                    property_name.as_str(),
                    exp0,
                    properties,
                    adoption_span,
                );
                continue;
            }
            if properties.get(property_name.as_str()).is_some_and(|found| {
                group
                    .iter()
                    .all(|(_, e)| self.types_compatible(&found.return_type, &e.return_type))
            }) {
                continue;
            }
            self.errors.push(errors::supertrait_property_ambiguity(
                trait_name,
                property_name.as_str(),
                &group[0].0,
                &group[1].0,
                adoption_span,
            ));
        }
    }

    /// Check that `methods` on a concrete type satisfy one abstract requirement from the trait graph.
    ///
    /// `via_trait` is the trait that originated the obligation (for diagnostics). Skips requirements that already have
    /// a default body on the trait (`has_body`).
    pub(super) fn check_impl_against_trait_method_requirement(
        &mut self,
        type_name: &str,
        via_trait: &str,
        method_name: &str,
        method_info: &MethodInfo,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        adoption_span: Span,
    ) {
        if method_info.has_body {
            return;
        }
        match method_overloads.get(method_name) {
            None => self
                .errors
                .push(errors::missing_trait_method(via_trait, method_name, adoption_span)),
            Some(found_group) => {
                if !found_group.iter().any(|found| {
                    Self::method_trait_target_matches(found, via_trait)
                        && self.method_sigs_compatible(method_info, found)
                }) {
                    let expected_sig = self.method_sig_string_named(method_name, method_info);
                    let found_sig = found_group
                        .first()
                        .map(|found| self.method_sig_string_named(method_name, found))
                        .unwrap_or_else(|| "<missing>".to_string());
                    self.errors.push(errors::trait_method_signature_mismatch(
                        via_trait,
                        type_name,
                        method_name,
                        &expected_sig,
                        &found_sig,
                        adoption_span,
                    ));
                }
            }
        }
    }

    /// Enforce abstract methods from `trait_name` and its supertraits on a concrete type's method map (RFC 042).
    pub(super) fn enforce_trait_abstract_methods(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let self_ty = self.trait_conformance_self_type(type_name);
        let concrete_method_overloads = self.concretize_trait_conformance_method_overloads(method_overloads, &self_ty);
        let grouped = self
            .grouped_trait_abstract_method_obligations(trait_name, trait_args.map(|args| (trait_info, args)))
            .into_iter()
            .map(|(method_name, entries)| {
                (
                    method_name,
                    entries
                        .into_iter()
                        .map(|(origin, info)| (origin, self.concretize_trait_method_requirement(&info, &self_ty)))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut method_names: Vec<String> = grouped.keys().cloned().collect();
        method_names.sort();
        for method_name in method_names {
            let Some(group) = grouped.get(&method_name) else {
                continue;
            };
            if group.is_empty() {
                continue;
            }
            let exp0 = &group[0].1;
            if group.len() == 1 {
                self.check_impl_against_trait_method_requirement(
                    type_name,
                    &group[0].0,
                    method_name.as_str(),
                    exp0,
                    &concrete_method_overloads,
                    adoption_span,
                );
                continue;
            }
            let all_mutually_compat = group
                .iter()
                .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
            if !all_mutually_compat {
                self.errors.push(errors::trait_conflict(
                    &group[0].0,
                    &group[1].0,
                    method_name.as_str(),
                    adoption_span,
                ));
                continue;
            }
            let all_identical = group.iter().all(|(_, e)| method_infos_identical(exp0, e));
            if all_identical {
                self.check_impl_against_trait_method_requirement(
                    type_name,
                    &group[0].0,
                    method_name.as_str(),
                    exp0,
                    &concrete_method_overloads,
                    adoption_span,
                );
                continue;
            }
            let satisfies_all = concrete_method_overloads
                .get(method_name.as_str())
                .is_some_and(|found_group| {
                    found_group
                        .iter()
                        .any(|found| group.iter().all(|(_, e)| self.method_sigs_compatible(e, found)))
                });
            if satisfies_all {
                continue;
            }
            if let Some(tm) = trait_info.methods.get(method_name.as_str())
                && tm.has_body
            {
                continue;
            }
            self.errors.push(errors::supertrait_method_ambiguity(
                trait_name,
                method_name.as_str(),
                &group[0].0,
                &group[1].0,
                adoption_span,
            ));
        }
    }

    /// If a concrete type overrides a trait default method, the override must remain signature-compatible.
    pub(super) fn enforce_trait_default_method_overrides(
        &mut self,
        type_name: &str,
        trait_name: &str,
        trait_info: &TraitInfo,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let adoption = ResolvedTraitAdoption {
            name: trait_name.to_string(),
            info: trait_info.clone(),
            args: trait_args.map(|args| args.to_vec()).unwrap_or_default(),
            module_path: None,
            span: adoption_span,
        };
        let self_ty = self.trait_conformance_self_type(type_name);
        let concrete_method_overloads = self.concretize_trait_conformance_method_overloads(method_overloads, &self_ty);
        for entry in self.trait_method_entries_for_adoption(&adoption) {
            if !entry.info.has_body {
                continue;
            }
            let Some(found_group) = concrete_method_overloads.get(&entry.method_name) else {
                continue;
            };
            let expected = self.concretize_trait_method_requirement(&entry.info, &self_ty);
            if !found_group
                .iter()
                .any(|found| self.method_sigs_compatible(&expected, found))
            {
                let expected_sig = self.method_sig_string_named(&entry.method_name, &expected);
                let found_sig = found_group
                    .first()
                    .map(|found| self.method_sig_string_named(&entry.method_name, found))
                    .unwrap_or_else(|| "<missing>".to_string());
                self.errors.push(errors::trait_method_signature_mismatch(
                    &entry.origin_trait,
                    type_name,
                    &entry.method_name,
                    &expected_sig,
                    &found_sig,
                    adoption_span,
                ));
            }
        }
    }

    /// If a trait method partial explicitly overrides an inherited trait method, its projected signature must remain
    /// compatible with the inherited surface it shadows.
    pub(super) fn validate_trait_partial_inherited_overrides(
        &mut self,
        trait_name: &str,
        partials: &[Spanned<MethodPartialDecl>],
    ) {
        if partials.is_empty() {
            return;
        }
        let Some(trait_info) = self.lookup_trait_info(trait_name).cloned() else {
            return;
        };
        for partial in partials {
            let Some(found) = trait_info.methods.get(&partial.node.name).cloned() else {
                continue;
            };
            for (origin_trait, supertrait_args) in self.semantic_supertrait_closure(trait_name) {
                let Some(supertrait_info) = self.lookup_semantic_trait_info(origin_trait.as_str()) else {
                    continue;
                };
                let Some(expected) = supertrait_info.methods.get(&partial.node.name) else {
                    continue;
                };
                let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
                let expected = substitute_method_info(expected, &subst);
                if self.method_sigs_compatible(&expected, &found) {
                    continue;
                }
                let expected_sig = self.method_sig_string_named(&partial.node.name, &expected);
                let found_sig = self.method_sig_string_named(&partial.node.name, &found);
                self.errors.push(errors::trait_method_signature_mismatch(
                    &origin_trait,
                    trait_name,
                    &partial.node.name,
                    &expected_sig,
                    &found_sig,
                    partial.span,
                ));
            }
        }
    }

    /// Render trait type arguments into a stable diagnostic and duplicate-detection key.
    pub(super) fn trait_args_key(args: &[ResolvedType]) -> String {
        args.iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Reject identical generic trait instantiations adopted more than once by one type.
    pub(super) fn validate_duplicate_trait_instantiations(&mut self, adoptions: &[ResolvedTraitAdoption]) {
        let mut seen: HashMap<(String, String), Span> = HashMap::new();
        for adoption in adoptions {
            let args_key = Self::trait_args_key(&adoption.args);
            let key = (adoption.name.clone(), args_key.clone());
            if let BindingRegistration::Collision { existing } = register_binding(&mut seen, key, adoption.span) {
                self.errors.push(errors::duplicate_trait_instantiation(
                    &adoption.name,
                    &args_key,
                    existing,
                    adoption.span,
                ));
            }
        }
    }

    /// Reject same-name method obligations that come from unrelated adopted trait families.
    pub(super) fn validate_cross_trait_method_collisions(
        &mut self,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
    ) {
        let mut by_method: HashMap<String, Vec<(String, MethodInfo, Span)>> = HashMap::new();
        for adoption in adoptions {
            for entry in self.trait_method_entries_for_adoption(adoption) {
                by_method
                    .entry(entry.method_name)
                    .or_default()
                    .push((entry.origin_trait, entry.info, adoption.span));
            }
        }
        for (method, entries) in by_method {
            let filtered_entries = self.filter_supertrait_dominated_entries(
                entries
                    .iter()
                    .map(|(origin, info, _)| (origin.clone(), info.clone()))
                    .collect(),
            );
            let mut origins: Vec<(String, Span)> = Vec::new();
            for (origin, _) in filtered_entries {
                if origins.iter().any(|(existing, _)| existing == &origin) {
                    continue;
                }
                let span = entries
                    .iter()
                    .find(|(entry_origin, _, _)| entry_origin == &origin)
                    .map(|(_, _, span)| *span)
                    .unwrap_or_default();
                origins.push((origin, span));
            }
            if origins.len() > 1 {
                if let Some(overloads) = method_overloads.get(&method)
                    && origins.iter().all(|(origin, _)| {
                        overloads.iter().any(|method_info| {
                            method_info
                                .trait_target
                                .as_ref()
                                .is_some_and(|target| target.name == *origin)
                        })
                    })
                {
                    continue;
                }
                let (left, span) = &origins[0];
                let (right, _) = &origins[1];
                self.errors
                    .push(errors::cross_trait_method_collision(left, right, &method, *span));
            }
        }
    }

    /// Return whether a collected method may satisfy the named trait target.
    pub(super) fn method_trait_target_matches(method_info: &MethodInfo, trait_name: &str) -> bool {
        method_info
            .trait_target
            .as_ref()
            .is_none_or(|target| target.name == trait_name)
    }

    /// Group source spans by method name so overload diagnostics can point at the matching declaration.
    pub(super) fn method_decl_spans_by_name(methods: &[Spanned<MethodDecl>]) -> HashMap<String, Vec<Span>> {
        let mut spans: HashMap<String, Vec<Span>> = HashMap::new();
        for method in methods {
            spans.entry(method.node.name.clone()).or_default().push(method.span);
        }
        spans
    }

    /// Refuse ordinary method overload declarations whose positional call surfaces conflict under RFC 174.
    ///
    /// Same-family RFC 025 trait implementations remain the one exception: two matching obligations may share an
    /// argument surface because their adopted trait instantiations retain the established result-type tie-break.
    pub(super) fn validate_method_overload_distinctness(
        &mut self,
        type_name: &str,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        method_spans: &HashMap<String, Vec<Span>>,
    ) {
        let mut obligations_by_method: HashMap<String, Vec<(String, MethodInfo)>> = HashMap::new();
        for adoption in adoptions {
            for entry in self.trait_method_entries_for_adoption(adoption) {
                obligations_by_method
                    .entry(entry.method_name)
                    .or_default()
                    .push((entry.origin_trait, entry.info));
            }
        }

        for (method_name, overloads) in method_overloads {
            for surface in [MemberBindingSurface::Type, MemberBindingSurface::Instance] {
                let surface_overloads = overloads
                    .iter()
                    .enumerate()
                    .filter(|(_, method)| surface.accepts_method(method))
                    .collect::<Vec<_>>();
                if surface_overloads.len() <= 1 {
                    continue;
                }
                let obligations = obligations_by_method
                    .get(method_name)
                    .into_iter()
                    .flatten()
                    .filter(|(_, method)| surface.accepts_method(method))
                    .collect::<Vec<_>>();
                let mut matched_obligations = vec![false; obligations.len()];
                let mut overload_matches: Vec<Option<usize>> = vec![None; surface_overloads.len()];
                for (idx, (_, overload)) in surface_overloads.iter().enumerate() {
                    let matched_index = obligations
                        .iter()
                        .enumerate()
                        .find(|(obligation_idx, (origin_trait, expected))| {
                            !matched_obligations[*obligation_idx]
                                && Self::method_trait_target_matches(overload, origin_trait)
                                && self.method_sigs_compatible(expected, overload)
                        })
                        .map(|(obligation_idx, _)| obligation_idx);
                    if let Some(obligation_idx) = matched_index {
                        matched_obligations[obligation_idx] = true;
                        overload_matches[idx] = Some(obligation_idx);
                    }
                }
                for later_idx in 1..surface_overloads.len() {
                    let Some((earlier_idx, (earlier_source_idx, _))) = surface_overloads
                        .iter()
                        .take(later_idx)
                        .enumerate()
                        .find(|(earlier_idx, (_, earlier))| {
                            let (_, later) = surface_overloads[later_idx];
                            let trait_pair = overload_matches[*earlier_idx].is_some()
                                && overload_matches[later_idx].is_some()
                                && overload_matches[*earlier_idx] != overload_matches[later_idx];
                            !trait_pair && self.method_overload_shapes_conflict(earlier, later)
                        })
                    else {
                        continue;
                    };
                    let later_source_idx = surface_overloads[later_idx].0;
                    let spans = method_spans.get(method_name);
                    let earlier_span = spans
                        .and_then(|spans| spans.get(*earlier_source_idx).copied())
                        .unwrap_or_default();
                    let later_span = spans
                        .and_then(|spans| spans.get(later_source_idx).copied())
                        .unwrap_or_default();
                    debug_assert!(earlier_idx < later_idx);
                    self.errors.push(errors::conflicting_method_overload(
                        type_name,
                        method_name,
                        earlier_span,
                        later_span,
                    ));
                }
            }
        }
    }

    /// Run RFC 025 validation for generic trait adoptions and same-name concrete methods.
    pub(super) fn validate_multi_instantiation_trait_surface(
        &mut self,
        type_name: &str,
        adoptions: &[ResolvedTraitAdoption],
        method_overloads: &HashMap<String, Vec<MethodInfo>>,
        method_spans: &HashMap<String, Vec<Span>>,
    ) {
        self.validate_duplicate_trait_instantiations(adoptions);
        self.validate_cross_trait_method_collisions(adoptions, method_overloads);
        self.validate_method_overload_distinctness(type_name, adoptions, method_overloads, method_spans);
    }

    /// Refuse every explicit `Awaitable[T]` adoption on a source-declared nominal type: a model, class, enum or
    /// newtype.
    ///
    /// RFC 039 admits three await realizations -- a Rust-backed future, a stdlib task handle, and a wrapper delegating
    /// to an awaitable member -- and requires that nothing can write `with Awaitable[T]` and then fail to build. The
    /// wrapper path has no realization: the adoption would be emitted as an implementation of a trait that is only a
    /// bound (`incan_std_async::task::Awaitable<T>`, a blanket over `IntoFuture`, which rustc refuses a second
    /// implementation of), and a model, class or enum always derives `Clone` and `Debug`, which the awaitable handle
    /// it would wrap (`JoinHandle[T]`) cannot be. Rusttype adoptions are gated the same way by
    /// `awaitable_future_bridge_blocked`. A program awaits the handle itself, or takes the awaitable through a generic
    /// bound, which is the surface the runtime realizes (#1711).
    pub(super) fn refuse_awaitable_adoptions(&mut self, type_name: &str, adoptions: &[ResolvedTraitAdoption]) {
        let awaitable_name = builtin_traits::as_str(TraitId::Awaitable);
        for adoption in adoptions.iter().filter(|adoption| adoption.name == awaitable_name) {
            let expected_output = adoption
                .args
                .first()
                .map_or_else(|| "T".to_string(), ToString::to_string);
            self.errors.push(errors::awaitable_adoption_not_realized(
                type_name,
                &expected_output,
                adoption.span,
            ));
        }
    }
}
