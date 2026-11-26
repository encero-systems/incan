//! Collection of traits and capabilities, and the supertrait graph.

use super::*;

impl TypeChecker {
    /// Register a trait declaration with its method signatures, supertraits, and requirements.
    pub(super) fn collect_trait(&mut self, tr: &TraitDecl, span: Span) {
        let mut method_overloads = collect_method_overloads(&tr.methods, self, None, &tr.type_params, false);
        // A method declared in a trait takes the trait's visibility: public for a `pub` trait, and otherwise the
        // trait's module, the only one that can name it (#2570).
        for method in method_overloads.values_mut().flatten() {
            method.scope = super::method_visibility::trait_method_scope(tr.visibility);
        }
        let mut methods = collect_methods_from_overloads(&method_overloads);
        let properties = collect_properties(&tr.properties, self, None, &tr.type_params);
        let method_aliases = collect_method_aliases(&tr.method_aliases, self, &mut methods, &mut method_overloads);
        self.collect_method_partials(&tr.name, &tr.method_partials, &mut methods, &mut method_overloads);
        let requires = self.extract_requires(&tr.decorators);
        let symbol_id = self.symbols.define(Symbol {
            name: tr.name.clone(),
            kind: SymbolKind::Trait(TraitInfo {
                type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                supertraits: Vec::new(),
                methods,
                method_aliases,
                properties,
                requires,
            }),
            span,
            scope: 0,
        });
        if self.symbols.is_active_lookup_binding(symbol_id) && !tr.traits.is_empty() {
            self.pending_trait_supertraits.push((symbol_id, tr.traits.clone()));
        }
    }

    /// Register a `capability` declaration with its description, typed scope dimensions, and requirements (RFC 104).
    ///
    /// The scope dimensions are resolved here because they are ordinary type expressions, but the `requires` entries
    /// are only recorded: a capability may require one declared later in the same module, so resolving them during
    /// collection would make declaration order significant.
    pub(super) fn collect_capability(&mut self, cap: &CapabilityDecl, span: Span) {
        let description = cap
            .description
            .as_ref()
            .and_then(|expr| capability_description_text(&expr.node))
            .map(str::to_string);

        let scope = cap
            .scope
            .iter()
            .map(|dim| (dim.node.name.clone(), self.resolve_type_checked(&dim.node.ty)))
            .collect();

        let requires = cap
            .requires
            .iter()
            .filter_map(|entry| {
                dotted_path_segments(&entry.node).map(|path| CapabilityRequirement { path, span: entry.span })
            })
            .collect();

        self.symbols.define(Symbol {
            name: cap.name.clone(),
            kind: SymbolKind::Capability(CapabilityInfo {
                description,
                scope,
                requires,
                is_public: matches!(cap.visibility, Visibility::Public),
            }),
            span,
            scope: 0,
        });
    }

    /// Resolve one `with` supertrait bound to `(trait_name, type_arguments)` after validation (RFC 042).
    pub(in crate::typechecker) fn resolve_trait_supertrait_bound(
        &mut self,
        bound: &Spanned<TraitBound>,
    ) -> Option<(String, Vec<ResolvedType>)> {
        let trait_name = self.resolve_trait_bound_name(&bound.node.name, bound.span);
        let args = bound
            .node
            .type_args
            .iter()
            .map(|arg| self.resolve_type_checked(arg))
            .collect::<Vec<_>>();
        // A foreign trait does not have an Incan declaration to collect. Keep its imported identity so the
        // transitive bound graph and Rust lowering both retain the real supertrait. As with direct Rust trait
        // adoption, absent metadata defers validation to rustc; positive non-trait metadata must never be accepted.
        if let Some(symbol) = self.lookup_symbol(&trait_name)
            && let SymbolKind::RustItem(info) = &symbol.kind
        {
            if info
                .metadata
                .as_ref()
                .is_some_and(|metadata| !matches!(metadata.kind, incan_lang::interop::RustItemKind::Trait(_)))
            {
                self.errors
                    .push(errors::supertrait_bound_not_trait(&trait_name, bound.span));
                return None;
            }
            // Rust trait metadata currently carries associated members, but no generic arity. Rustc therefore
            // checks foreign argument counts; native Incan trait arity remains checked below.
            return Some((format!("::{}", info.path.trim_start_matches("::")), args));
        }
        let trait_info = if let Some(info) = self.lookup_trait_info(&trait_name).cloned() {
            info
        } else if let Some((hidden_name, info)) = self.resolve_imported_trait_bound_symbol(&bound.node.name, bound.span)
        {
            return self.validate_supertrait_bound(hidden_name, args, info, bound.span);
        } else {
            self.errors
                .push(errors::supertrait_bound_not_trait(&trait_name, bound.span));
            return None;
        };
        self.validate_supertrait_bound(trait_name, args, trait_info, bound.span)
    }

    /// Define a hidden trait symbol for an imported supertrait bound that has not been collected as a declaration.
    pub(super) fn resolve_imported_trait_bound_symbol(
        &mut self,
        name: &str,
        span: Span,
    ) -> Option<(String, TraitInfo)> {
        let module_path = self.trait_bound_module_path(name)?;
        let trait_name = self
            .import_binding_path(name)
            .and_then(|path| path.last())
            .cloned()
            .unwrap_or_else(|| name.rsplit('.').next().unwrap_or(name).to_string());
        let info = self.lookup_imported_module_trait(&module_path, &trait_name)?;
        let symbol_name = name.to_string();
        let identity = self.package_module_trait_identity(name);
        self.define_hidden_trait_symbol_with_identity(&symbol_name, info.clone(), identity, span);
        Some((symbol_name, info))
    }

    /// Validate a resolved supertrait bound against the target trait arity.
    pub(super) fn validate_supertrait_bound(
        &mut self,
        trait_name: String,
        args: Vec<ResolvedType>,
        trait_info: TraitInfo,
        span: Span,
    ) -> Option<(String, Vec<ResolvedType>)> {
        let expected_arity = trait_info.type_params.len();
        if args.len() != expected_arity {
            self.errors.push(errors::supertrait_bound_arity_mismatch(
                &trait_name,
                expected_arity,
                args.len(),
                span,
            ));
            return None;
        }
        Some((trait_name, args))
    }

    /// Resolve queued trait `with` bounds now that all types and traits exist in the symbol table (RFC 042).
    pub fn resolve_pending_trait_supertraits(&mut self) {
        let pending = std::mem::take(&mut self.pending_trait_supertraits);
        for (symbol_id, bounds) in pending {
            let mut supertraits: Vec<(String, Vec<ResolvedType>)> = Vec::new();
            for bound in &bounds {
                if let Some(entry) = self.resolve_trait_supertrait_bound(bound) {
                    supertraits.push(entry);
                }
            }
            let Some(sym) = self.symbols.get_mut(symbol_id) else {
                continue;
            };
            let SymbolKind::Trait(info) = &mut sym.kind else {
                continue;
            };
            info.supertraits = supertraits;
        }
    }

    /// After all declarations are collected: detect supertrait cycles and fill `supertrait_closure`.
    pub fn finalize_supertrait_graph(&mut self) {
        self.supertrait_closure.clear();
        let edges = self.supertrait_name_adjacency();
        let trait_names: Vec<String> = self
            .symbols
            .all_symbols()
            .iter()
            .filter_map(|sym| matches!(sym.kind, SymbolKind::Trait(_)).then_some(sym.name.clone()))
            .collect();
        if let Some(cycle) = find_supertrait_cycle_path(&edges) {
            let span = cycle
                .first()
                .and_then(|name| self.lookup_symbol(name))
                .map(|sym| sym.span)
                .unwrap_or_default();
            self.errors.push(errors::supertrait_cycle(&cycle, span));
            for name in trait_names {
                self.supertrait_closure.insert(name, Vec::new());
            }
            return;
        }
        for name in trait_names {
            let closure = self.expand_supertraits_transitively(&name);
            self.supertrait_closure.insert(name, closure);
        }
    }

    pub(super) fn supertrait_name_adjacency(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                let names: Vec<String> = info.supertraits.iter().map(|(n, _)| n.clone()).collect();
                map.insert(sym.name.clone(), names);
            }
        }
        map
    }

    /// Transitive supertraits of `trait_name`, with type arguments substituted along each edge.
    pub(super) fn expand_supertraits_transitively(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let mut seen = HashSet::new();
        let mut work: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let Some(root) = self.lookup_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());
        while let Some((sup_name, sup_args)) = work.pop() {
            let key = format!(
                "{sup_name}<{}>",
                sup_args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((sup_name.clone(), sup_args.clone()));
            let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                continue;
            };
            let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
            for (ss_name, ss_args) in &sup_info.supertraits {
                let mapped: Vec<ResolvedType> = ss_args.iter().map(|t| substitute_resolved_type(t, &subst)).collect();
                work.push((ss_name.clone(), mapped));
            }
        }
        result
    }

    /// Merge `@requires` fields from transitive supertraits into each trait symbol (RFC 042).
    ///
    /// Uses each trait's **explicit** `@requires` from a snapshot taken before merging, so order does not matter.
    /// Incompatible requirements for the same field name emit [`errors::supertrait_requires_conflict`].
    pub fn merge_supertrait_requires_into_traits(&mut self) {
        let mut explicit: HashMap<String, Vec<(String, ResolvedType)>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                explicit.insert(sym.name.clone(), info.requires.clone());
            }
        }

        let trait_names: Vec<String> = explicit.keys().cloned().collect();
        let mut updates: Vec<(String, Vec<(String, ResolvedType)>)> = Vec::new();

        for tname in trait_names {
            let span = self
                .symbols
                .lookup(&tname)
                .and_then(|id| self.symbols.get(id))
                .map(|s| s.span)
                .unwrap_or_default();

            let mut merged: HashMap<String, ResolvedType> =
                explicit.get(&tname).cloned().unwrap_or_default().into_iter().collect();

            let closure = self.supertrait_closure.get(tname.as_str()).cloned().unwrap_or_default();

            for (sup_name, sup_args) in closure {
                let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                    continue;
                };
                let sup_req = explicit.get(&sup_name).cloned().unwrap_or_default();
                let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
                for (field, ty) in sup_req {
                    let inst = substitute_resolved_type(&ty, &subst);
                    match merged.get(&field) {
                        None => {
                            merged.insert(field, inst);
                        }
                        Some(existing) => {
                            if existing == &inst {
                                continue;
                            }
                            if self.types_compatible(existing, &inst) || self.types_compatible(&inst, existing) {
                                continue;
                            }
                            self.errors.push(errors::supertrait_requires_conflict(
                                &tname,
                                &field,
                                &existing.to_string(),
                                &inst.to_string(),
                                span,
                            ));
                        }
                    }
                }
            }

            let mut req_vec: Vec<(String, ResolvedType)> = merged.into_iter().collect();
            req_vec.sort_by(|a, b| a.0.cmp(&b.0));
            updates.push((tname, req_vec));
        }

        for (tname, requires) in updates {
            if let Some(id) = self.symbols.lookup(&tname)
                && let Some(sym) = self.symbols.get_mut(id)
                && let SymbolKind::Trait(info) = &mut sym.kind
            {
                info.requires = requires;
            }
        }
    }
}

/// Returns one simple cycle (trait names), if the directed supertrait graph has a cycle.
pub(super) fn find_supertrait_cycle_path(edges: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut nodes: HashSet<String> = HashSet::new();
    for (k, vs) in edges {
        nodes.insert(k.clone());
        for v in vs {
            nodes.insert(v.clone());
        }
    }
    let mut color: HashMap<String, u8> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for start in nodes {
        if color.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        stack.clear();
        if let Some(cycle) = dfs_supertrait_cycle(&start, edges, &mut color, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

pub(super) fn dfs_supertrait_cycle(
    n: &str,
    edges: &HashMap<String, Vec<String>>,
    color: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    match color.get(n).copied().unwrap_or(0) {
        1 => {
            let idx = stack.iter().position(|x| x == n)?;
            Some(stack[idx..].to_vec())
        }
        2 => None,
        _ => {
            color.insert(n.to_string(), 1);
            stack.push(n.to_string());
            for succ in edges.get(n).into_iter().flatten() {
                if let Some(c) = dfs_supertrait_cycle(succ, edges, color, stack) {
                    return Some(c);
                }
            }
            stack.pop();
            color.insert(n.to_string(), 2);
            None
        }
    }
}
