//! Trait method and property resolution for adoptions.

use super::*;

impl TypeChecker {
    /// Resolve a trait method visible when a concrete type adopts `adopted_trait`, including methods from transitive
    /// supertraits with type arguments substituted per the supertrait closure (RFC 042).
    ///
    /// Supertrait shadowing matches [`Self::grouped_trait_abstract_method_obligations`]: along a refinement chain, a
    /// subtrait's declaration dominates its supertrait's same-named method for lookup purposes.
    ///
    /// When multiple origins remain after filtering (diamond shapes), signatures must be mutually compatible; otherwise
    /// a [`errors::trait_conflict`] diagnostic is recorded and `None` is returned.
    pub(in crate::typechecker) fn trait_method_info_resolved(
        &mut self,
        adopted_trait: &str,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<MethodInfo> {
        let mut entries: Vec<(String, MethodInfo)> = Vec::new();
        if let Some(root) = self.lookup_semantic_trait_info(adopted_trait)
            && let Some(info) = root.methods.get(method)
        {
            entries.push((adopted_trait.to_string(), info.clone()));
        }
        let closure = self.semantic_supertrait_closure(adopted_trait);
        self.load_builtin_supertrait_sources(&closure);
        for (supertrait_name, supertrait_args) in closure {
            let Some(supertrait_info) = self.supertrait_info_for_methods(supertrait_name.as_str()) else {
                continue;
            };
            let Some(info) = supertrait_info.methods.get(method) else {
                continue;
            };
            let subst = type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
            entries.push((supertrait_name, substitute_method_info(info, &subst)));
        }
        let filtered = self.filter_supertrait_dominated_entries(entries);
        // A compiler-owned trait can resolve to an empty symbol-table stub, which shadows the real contract and leaves
        // every derive-provided method unresolvable; the standard package's declaration of the trait carries it.
        if filtered.is_empty()
            && let Some(full_trait) = self.standard_trait(adopted_trait)
            && let Some(info) = full_trait.methods.get(method)
        {
            return Some(info.clone());
        }
        match filtered.as_slice() {
            [] => None,
            [(_, info)] => Some(info.clone()),
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest
                    .iter()
                    .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
                if !all_mutually_compat {
                    self.errors
                        .push(errors::trait_conflict(&rest[0].0, &rest[1].0, method, ambiguity_span));
                    return None;
                }
                let all_identical =
                    !exp0.has_body && rest.iter().all(|(_, e)| !e.has_body && method_infos_identical(exp0, e));
                if !all_identical {
                    self.errors.push(errors::supertrait_method_ambiguity(
                        adopted_trait,
                        method,
                        &rest[0].0,
                        &rest[1].0,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(exp0.clone())
            }
        }
    }

    /// Resolve one method through a concrete trait adoption that may include generic trait arguments.
    pub(in crate::typechecker) fn trait_method_info_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<MethodInfo> {
        self.trait_method_entry_resolved_for_adoption(adoption, method, ambiguity_span)
            .map(|entry| entry.info)
    }

    /// Resolve one method entry through a concrete trait adoption, preserving the originating trait provenance.
    /// A builtin identity names a methodless placeholder; its standard declaration remains authoritative even when
    /// an imported adoption retains a provider-relative module path.
    pub(in crate::typechecker) fn trait_method_entry_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        method: &str,
        ambiguity_span: Span,
    ) -> Option<TraitMethodEntry> {
        if adoption.module_path.is_none() && adoption.type_args.is_empty() {
            return self
                .trait_method_info_resolved(&adoption.name, method, ambiguity_span)
                .map(|info| TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: adoption.name.clone(),
                    origin_type_args: Vec::new(),
                    origin_module_path: None,
                    implementation_type_params: adoption.implementation_type_params.clone(),
                    info,
                });
        }
        let root = self.trait_adoption_root_info(adoption)?;
        self.load_builtin_supertrait_sources(&root.supertraits);
        let instantiated = if adoption.type_args.is_empty() {
            root
        } else {
            self.instantiate_trait_info(&root, &adoption.type_args)
        };
        let resolved = ResolvedTraitAdoption {
            name: self.adoption_trait_name_here(adoption),
            info: instantiated,
            args: adoption.type_args.clone(),
            module_path: adoption.module_path.clone(),
            span: ambiguity_span,
        };
        let entries = self.trait_method_entries_for_adoption(&resolved);
        let matching: Vec<TraitMethodMatch> = entries
            .into_iter()
            .filter(|entry| entry.method_name == method)
            .map(|entry| {
                (
                    entry.origin_trait,
                    entry.origin_module_path,
                    entry.origin_type_args,
                    entry.info,
                )
            })
            .collect();
        let filtered = self.filter_supertrait_dominated_entries(
            matching
                .iter()
                .map(|(origin, _, _, info)| (origin.clone(), info.clone()))
                .collect(),
        );
        match filtered.as_slice() {
            [] => None,
            [(origin_trait, info)] => {
                let (origin_type_args, origin_module_path) = matching
                    .iter()
                    .find(|(origin, _, _, candidate)| {
                        origin == origin_trait && self.method_sigs_compatible(candidate, info)
                    })
                    .map(|(_, module_path, args, _)| (args.clone(), module_path.clone()))
                    .unwrap_or_default();
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: origin_trait.clone(),
                    origin_type_args,
                    origin_module_path,
                    implementation_type_params: adoption.implementation_type_params.clone(),
                    info: info.clone(),
                })
            }
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest
                    .iter()
                    .all(|(_, e)| self.method_sigs_compatible(exp0, e) && self.method_sigs_compatible(e, exp0));
                if !all_mutually_compat {
                    self.errors
                        .push(errors::trait_conflict(&rest[0].0, &rest[1].0, method, ambiguity_span));
                    return None;
                }
                let all_identical =
                    !exp0.has_body && rest.iter().all(|(_, e)| !e.has_body && method_infos_identical(exp0, e));
                if !all_identical {
                    self.errors.push(errors::supertrait_method_ambiguity(
                        &adoption.name,
                        method,
                        &rest[0].0,
                        &rest[1].0,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(TraitMethodEntry {
                    method_name: method.to_string(),
                    origin_trait: rest[0].0.clone(),
                    origin_type_args: matching
                        .iter()
                        .find(|(origin, _, _, candidate)| {
                            origin == &rest[0].0 && self.method_sigs_compatible(candidate, exp0)
                        })
                        .map(|(_, _, args, _)| args.clone())
                        .unwrap_or_default(),
                    origin_module_path: matching
                        .iter()
                        .find(|(origin, _, _, candidate)| {
                            origin == &rest[0].0 && self.method_sigs_compatible(candidate, exp0)
                        })
                        .and_then(|(_, module_path, _, _)| module_path.clone()),
                    implementation_type_params: adoption.implementation_type_params.clone(),
                    info: exp0.clone(),
                })
            }
        }
    }

    /// Select an adopted trait's declaration before instantiation. Compiler-owned builtin stubs defer to their
    /// standard declaration; otherwise preserve adoption, provider-module and standard fallback precedence.
    pub(super) fn trait_adoption_root_info(&mut self, adoption: &TypeBoundInfo) -> Option<TraitInfo> {
        let builtin_stub = self
            .symbols
            .lookup(&adoption.name)
            .and_then(|symbol| self.symbols.identity_of(symbol))
            .is_some_and(|identity| matches!(identity.origin, incan_semantics_core::SymbolOrigin::Builtin));
        let standard = ((adoption.module_path.is_none() || builtin_stub)
            && incan_lang::lang::traits::from_str(&adoption.name).is_some())
        .then(|| self.standard_trait(&adoption.name))
        .flatten();
        standard
            .or_else(|| self.lookup_trait_adoption_info(adoption).cloned())
            .or_else(|| {
                let module_path = adoption.module_path.as_ref()?;
                let source_name = adoption
                    .source_name
                    .as_deref()
                    .or_else(|| adoption.name.rsplit('.').next())?;
                self.loaf_trait(module_path, source_name)
            })
            .or_else(|| self.standard_trait(&adoption.name))
    }

    /// Resolve a trait property visible through an adopted trait, including transitive supertraits.
    pub(in crate::typechecker) fn trait_property_info_resolved_for_adoption(
        &mut self,
        adoption: &TypeBoundInfo,
        property: &str,
        ambiguity_span: Span,
    ) -> Option<PropertyInfo> {
        let root = self.lookup_trait_adoption_info(adoption)?.clone();
        let root_info = if adoption.type_args.is_empty() {
            root
        } else {
            self.instantiate_trait_info(&root, &adoption.type_args)
        };
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        self.collect_instantiated_trait_property_entries(
            &adoption.name,
            &root_info,
            &adoption.type_args,
            &mut seen,
            &mut entries,
        );
        let matching: Vec<(String, PropertyInfo)> = entries
            .into_iter()
            .filter(|(name, _, _)| name == property)
            .map(|(_, origin, info)| (origin, info))
            .collect();
        let filtered = self.filter_supertrait_dominated_property_entries(matching);
        match filtered.as_slice() {
            [] => None,
            [(_, info)] => Some(info.clone()),
            rest => {
                let exp0 = &rest[0].1;
                let all_mutually_compat = rest.iter().all(|(_, e)| {
                    self.types_compatible(&exp0.return_type, &e.return_type)
                        && self.types_compatible(&e.return_type, &exp0.return_type)
                });
                if !all_mutually_compat {
                    self.errors.push(errors::trait_property_conflict(
                        &rest[0].0,
                        &rest[1].0,
                        property,
                        ambiguity_span,
                    ));
                    return None;
                }
                Some(exp0.clone())
            }
        }
    }
}
