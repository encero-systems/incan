//! Type, symbol, variable and trait lookup, including trait implementation and adoption queries.

use super::*;

impl TypeChecker {
    /// Look up a type by name and return its [`TypeInfo`], if known.
    ///
    /// ## Parameters
    /// - `name`: The type name to look up.
    ///
    /// ## Returns
    /// - `Some(&TypeInfo)`: If `name` resolves to a type symbol.
    /// - `None`: If the symbol is missing or not a type.
    ///
    /// ## Notes
    /// - This helper exists to flatten the common pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    ///   - `match sym.kind { SymbolKind::Type(info) => ... }`
    pub fn lookup_type_info(&self, name: &str) -> Option<&TypeInfo> {
        let Some(id) = self.symbols.lookup(name) else {
            return self.module_qualified_nominal_type_info(name);
        };
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a symbol by name and return a reference to it, if found.
    ///
    /// Collapses the common two-step pattern:
    ///   - `symbols.lookup(name)` → `Option<SymbolId>`
    ///   - `symbols.get(id)` → `Option<&Symbol>`
    pub fn lookup_symbol(&self, name: &str) -> Option<&Symbol> {
        self.check_environment().lookup(name).map(|binding| binding.symbol)
    }

    /// Return the generic-parameter name for a type-like placeholder visible in the current scope.
    ///
    /// During declaration collection, generic parameters are recorded as `TypeVar`, but inside function / method bodies
    /// they are introduced into the symbol table as scoped `Type(Builtin)` placeholders so annotations keep resolving.
    /// For operator and method permissiveness (RFC 023 additive backend inference), both representations should behave
    /// as "typevar-like" generic placeholders.
    pub fn generic_placeholder_name<'a>(&self, ty: &'a ResolvedType) -> Option<&'a str> {
        match ty {
            ResolvedType::TypeVar(name) => Some(name.as_str()),
            ResolvedType::Named(name) => {
                let sym = self.lookup_symbol(name)?;
                match &sym.kind {
                    SymbolKind::Type(TypeInfo::Builtin) if sym.scope > 0 => Some(name.as_str()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether `ty` is a generic placeholder (`TypeVar`) or an in-scope type-parameter placeholder.
    pub fn is_generic_placeholder_type(&self, ty: &ResolvedType) -> bool {
        self.generic_placeholder_name(ty).is_some()
    }

    /// Return declared type parameters and explicit trait adoptions for a semantic type.
    pub(super) fn semantic_type_params_and_adoptions(&self, type_name: &str) -> Option<(&[String], &[TypeBoundInfo])> {
        match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => Some((&model.type_params, &model.trait_adoptions)),
            TypeInfo::Class(class) => Some((&class.type_params, &class.trait_adoptions)),
            TypeInfo::Enum(enum_info) => Some((&enum_info.type_params, &enum_info.trait_adoptions)),
            TypeInfo::Newtype(newtype) => Some((&newtype.type_params, &newtype.trait_adoptions)),
            _ => None,
        }
    }

    /// Infer the concrete instantiation of `trait_name` for `type_name`, if the type adopts that trait.
    ///
    /// Explicit `with Trait[...]` arguments are authoritative. The older positional mapping remains as a fallback for
    /// nullary adoption metadata and derives.
    pub fn instantiated_trait_args_for_type(
        &self,
        type_name: &str,
        concrete_type_args: &[ResolvedType],
        trait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let (type_params, adopted) = self.semantic_type_params_and_adoptions(type_name)?;
        let concrete_subst = crate::resolved_type_subst::type_param_subst_map(type_params, concrete_type_args);

        for adopted_trait in adopted {
            let Some(adopted_info) = self.lookup_trait_adoption_info(adopted_trait) else {
                continue;
            };
            let direct_args: Vec<ResolvedType> = if adopted_trait.type_args.is_empty() {
                concrete_type_args
                    .iter()
                    .take(adopted_info.type_params.len())
                    .cloned()
                    .collect()
            } else {
                adopted_trait
                    .type_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &concrete_subst))
                    .collect()
            };
            if direct_args.len() != adopted_info.type_params.len() {
                continue;
            }

            if self.trait_name_matches(&adopted_trait.name, trait_name) {
                return Some(direct_args);
            }

            let closure = self.semantic_supertrait_closure(&adopted_trait.name);
            let subst = crate::resolved_type_subst::type_param_subst_map(&adopted_info.type_params, &direct_args);
            for (supertrait_name, supertrait_args) in closure {
                if !self.trait_name_matches(&supertrait_name, trait_name) {
                    continue;
                }
                let instantiated = supertrait_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                return Some(instantiated);
            }
        }
        None
    }

    /// Whether `supertrait_name` is `subtrait_name` itself or appears in its RFC 042 transitive supertrait closure.
    pub(super) fn trait_is_supertrait_of(&self, subtrait_name: &str, supertrait_name: &str) -> bool {
        if self.trait_name_matches(subtrait_name, supertrait_name) {
            return true;
        }
        self.semantic_supertrait_closure(subtrait_name)
            .iter()
            .any(|(name, _)| self.trait_name_matches(name, supertrait_name))
    }

    /// Return whether two visible trait spellings refer to the same source trait. Standard imports and retained
    /// public Loaf paths compare through the owning manifest's declaration identity before their source paths.
    pub(super) fn trait_name_matches(&self, candidate: &str, expected: &str) -> bool {
        if candidate == expected {
            return true;
        }
        let foreign_path = |name: &str| {
            if name.starts_with("::") {
                return Some(name.trim_start_matches("::").to_string());
            }
            self.lookup_symbol(name).and_then(|symbol| match &symbol.kind {
                SymbolKind::RustItem(info) => Some(info.path.trim_start_matches("::").to_string()),
                _ => None,
            })
        };
        match (foreign_path(candidate), foreign_path(expected)) {
            (Some(candidate), Some(expected)) => return candidate == expected,
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        match (
            self.resolve_bound_trait_path(candidate),
            self.resolve_bound_trait_path(expected),
        ) {
            (Some(candidate), Some(expected)) => {
                match (
                    self.loaf_member_identity(&candidate.0, &candidate.1)
                        .or_else(|| self.package_trait_declaration(&candidate.0, &candidate.1)),
                    self.loaf_member_identity(&expected.0, &expected.1)
                        .or_else(|| self.package_trait_declaration(&expected.0, &expected.1)),
                ) {
                    (Some(candidate), Some(expected)) => candidate == expected,
                    _ => candidate == expected,
                }
            }
            (Some(_), None) | (None, Some(_)) => false,
            (None, None) => {
                candidate.rsplit('.').next().unwrap_or(candidate) == expected.rsplit('.').next().unwrap_or(expected)
            }
        }
    }

    /// Return whether a compiler-owned derive provides the visible trait bound, including imported aliases.
    pub(super) fn builtin_derive_satisfies_trait(&self, derive: &str, trait_name: &str) -> bool {
        let Some(derive_id) = incan_lang::lang::derives::from_str(derive) else {
            return false;
        };
        let canonical_derive = incan_lang::lang::derives::as_str(derive_id);
        if canonical_derive == trait_name && self.import_binding_path(trait_name).is_none() {
            return self.lookup_semantic_trait_info(trait_name).is_some();
        }
        let Some((module_path, source_trait)) = self.resolve_bound_trait_path(trait_name) else {
            return false;
        };
        source_trait == canonical_derive
            && stdlib::trait_method_module_segments(canonical_derive)
                .is_some_and(|builtin_module_path| builtin_module_path == module_path)
            && self.lookup_semantic_trait_info(trait_name).is_some()
    }

    /// Instantiate `supertrait_name`'s type arguments when `subtrait_name` is known with `subtrait_args`.
    ///
    /// Used for trait-to-supertrait compatibility: a value typed `Subtrait[A1,...]` may appear where
    /// `Supertrait[B1,...]` is required when the supertrait edge is satisfied after substitution.
    pub(super) fn instantiated_supertrait_args(
        &self,
        subtrait_name: &str,
        subtrait_args: &[ResolvedType],
        supertrait_name: &str,
    ) -> Option<Vec<ResolvedType>> {
        let sub_info = self.lookup_semantic_trait_info(subtrait_name)?;
        if subtrait_args.len() != sub_info.type_params.len() {
            return None;
        }
        let subst = crate::resolved_type_subst::type_param_subst_map(&sub_info.type_params, subtrait_args);
        if self.trait_name_matches(subtrait_name, supertrait_name) {
            return Some(subtrait_args.to_vec());
        }
        let closure = self.semantic_supertrait_closure(subtrait_name);
        for (name, args) in &closure {
            if !self.trait_name_matches(name, supertrait_name) {
                continue;
            }
            let instantiated: Vec<ResolvedType> = args
                .iter()
                .map(|a| crate::resolved_type_subst::substitute_resolved_type(a, &subst))
                .collect();
            return Some(instantiated);
        }
        None
    }

    /// Whether a concrete named type declares adoption of `trait_name` (RFC 042: includes transitive supertraits).
    ///
    /// Also treats `@derive(T)` as implementing trait `T` when `T` is a visible trait symbol (e.g. `Clone`).
    pub fn type_implements_trait(&self, type_name: &str, trait_name: &str) -> bool {
        let Some(info) = self.lookup_semantic_type_info(type_name) else {
            return false;
        };
        let (adopted, derives) = match info {
            TypeInfo::Model(m) => (m.trait_adoptions.as_slice(), Some(m.derives.as_slice())),
            TypeInfo::Class(c) => (c.trait_adoptions.as_slice(), Some(c.derives.as_slice())),
            TypeInfo::Enum(e) => (e.trait_adoptions.as_slice(), Some(e.derives.as_slice())),
            TypeInfo::Newtype(n) => (n.trait_adoptions.as_slice(), Some(n.derives.as_slice())),
            _ => return false,
        };
        // The trait the bound names, as a module and a trait name: an adoption that records the module it came from is
        // compared by that identity, so a type declared in another module satisfies `serde.json.Serialize` whatever the
        // declaring module called the trait (`json.Serialize` through its own import, #1887).
        let bound_identity = self.resolve_bound_trait_path(trait_name);
        let package_trait = self.package_trait_identity_of_type(type_name, trait_name);
        // A `pub::` package trait is one declaration under every spelling a consumer gives it (an import alias, a
        // package alias export, a module binding), so an adoption and a bound that name it under different spellings
        // meet through the package's checked identity of each (#1561).
        let bound_package_trait = bound_identity
            .as_ref()
            .and_then(|(module_path, name)| self.package_trait_declaration(module_path, name));
        for t in adopted {
            if self.trait_name_matches(&t.name, trait_name)
                || (t.module_path.is_none()
                    && t.source_name
                        .as_deref()
                        .is_some_and(|source_name| self.trait_name_matches(source_name, trait_name)))
                || adoption_names_bound_identity(t, bound_identity.as_ref())
                || package_trait
                    .as_ref()
                    .is_some_and(|(module_path, name)| adoption_names_declaration(t, module_path, name))
                || bound_package_trait
                    .as_ref()
                    .is_some_and(|bound| self.adopted_package_trait(t).as_ref() == Some(bound))
            {
                return true;
            }
            if self
                .semantic_supertrait_closure(&t.name)
                .iter()
                .any(|(name, _)| self.trait_name_matches(name, trait_name))
            {
                return true;
            }
        }
        if let Some(derives) = derives
            && derives
                .iter()
                .any(|derive| self.builtin_derive_satisfies_trait(derive, trait_name))
        {
            return true;
        }
        false
    }

    /// Return the declaring module and name of the trait `trait_name` binds when it and the type `type_name` come from
    /// the same compiled package, in the package's own module paths, which its types' adoptions record.
    ///
    /// A consumer binds the trait under its own spelling (`Picker as Chooser`, a package alias `pub Chooser =
    /// Picker`, or `t.Picker` through `import pub::pickers as t`) and under the package's public namespace, while the
    /// package's types recorded their adoptions in the package's modules (#1561). A type the consumer reaches through
    /// the package's namespace rather than an import of its own (`t.Label(...)`) is named by its public spelling.
    pub(super) fn package_trait_identity_of_type(
        &self,
        type_name: &str,
        trait_name: &str,
    ) -> Option<(Vec<String>, String)> {
        let package_of = |name: &str| {
            let identity = self
                .symbols
                .lookup(name)
                .and_then(|id| self.symbols.identity_of(id))
                .or_else(|| self.public_library_type_identities.get(name)?.canonical.as_ref())
                .cloned()
                .or_else(|| {
                    let (module_path, declaration) = self.resolve_bound_trait_path(name)?;
                    self.package_trait_declaration(&module_path, &declaration)
                })?;
            match identity.origin {
                SymbolOrigin::Package { library, module_path } => {
                    Some((identity.kind, identity.declaration_name, library, module_path))
                }
                _ => None,
            }
        };
        let (trait_kind, declaration_name, trait_library, trait_module) = package_of(trait_name)?;
        let (_, _, type_library, _) = package_of(type_name)?;
        (trait_kind == SemanticSourceTargetKind::Trait && trait_library == type_library)
            .then_some((trait_module, declaration_name))
    }

    /// Return the checked identity of a trait in an admitted package or standard module, as its identity graph
    /// publishes it. Standard and public-package paths select the same declaring identity; an alias export retains
    /// its target. A canonical physical module path can also select that exact published declaration without
    /// requiring a consumer import. An unowned module or a non-trait member grants no adoption.
    pub(super) fn package_trait_declaration(&self, module_path: &[String], name: &str) -> Option<CanonicalSymbolId> {
        let (manifest, rest) = match module_path {
            [root, library, rest @ ..] if root == PUBLIC_LIBRARY_NAMESPACE => (self.loaves.manifest(library)?, rest),
            [root, rest @ ..] if root == "std" => (self.loaves.owner_of(module_path)?.artifact.manifest.as_ref(), rest),
            _ => return None,
        };
        let public_path = std::iter::once(manifest.name.clone())
            .chain(rest.iter().cloned())
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>();
        manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_path(&public_path)
            .or_else(|| {
                manifest
                    .contract_metadata
                    .identity_graph
                    .exports
                    .iter()
                    .find_map(|export| {
                        let identity = export.canonical.as_ref()?.hydrate()?;
                        matches!(&identity.origin, SymbolOrigin::Package { library, module_path }
                        if library == &manifest.name && module_path.as_slice() == rest)
                        .then_some(identity)
                        .filter(|identity| identity.declaration_name == name)
                    })
            })
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Trait)
    }

    /// Return the checked identity of the `pub::` package trait an adoption names, from the package module and the
    /// source name it recorded where the adopting type was declared.
    pub(super) fn adopted_package_trait(&self, adoption: &TypeBoundInfo) -> Option<CanonicalSymbolId> {
        let module_path = adoption.module_path.as_deref()?;
        let name = adoption
            .source_name
            .as_deref()
            .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
        self.package_trait_declaration(module_path, name)
    }

    /// Explicit `with Trait[...]` entries plus trait-like `@derive` entries and the automatic `Clone` of a `model`,
    /// `class` or `enum`, for method lookup on one of those.
    ///
    /// Every model, class and enum derives `Clone` without `@derive(Clone)`, so `value.clone()` resolves through the
    /// builtin trait whether or not the derive is spelled (#1870).
    pub fn trait_adoptions_for_type_methods(
        &self,
        adopted: &[TypeBoundInfo],
        derives: &[String],
    ) -> Vec<TypeBoundInfo> {
        let automatic_clone = builtin_derives::as_str(DeriveId::Clone);
        self.with_derive_trait_adoptions(adopted, derives.iter().map(String::as_str).chain([automatic_clone]))
    }

    /// Explicit `with Trait[...]` entries of a newtype plus trait-like `@derive` entries and its automatic `Clone`, for
    /// method lookup on it.
    ///
    /// A newtype derives `Clone` automatically when its underlying type implements it (#1754), so `value.clone()`
    /// resolves whenever the derive relation says the newtype is `Clone` (#1870).
    pub(in crate::typechecker) fn newtype_trait_adoptions_for_type_methods(
        &self,
        name: &str,
        info: &NewtypeInfo,
    ) -> Vec<TypeBoundInfo> {
        let clone = builtin_derives::as_str(DeriveId::Clone);
        let automatic_clone = (!info.is_rusttype
            && self.derive_support(&ResolvedType::Named(name.to_string()), DeriveId::Clone)
                == derive_requirements::DeriveSupport::Supported)
            .then_some(clone);
        self.with_derive_trait_adoptions(
            &info.trait_adoptions,
            info.derives.iter().map(String::as_str).chain(automatic_clone),
        )
    }

    /// Append to `adopted` each derive in `derives` that names a trait in scope, once, as a trait adoption.
    pub(super) fn with_derive_trait_adoptions<'a>(
        &self,
        adopted: &[TypeBoundInfo],
        derives: impl Iterator<Item = &'a str>,
    ) -> Vec<TypeBoundInfo> {
        let mut out = adopted.to_vec();
        for derive in derives {
            if self.lookup_semantic_trait_info(derive).is_some() && !out.iter().any(|adoption| adoption.name == derive)
            {
                out.push(TypeBoundInfo {
                    name: derive.to_string(),
                    source_name: None,
                    type_args: Vec::new(),
                    module_path: None,
                    implementation_type_params: Vec::new(),
                    inferred: false,
                });
            }
        }
        out
    }

    /// Look up a variable binding by name (in any scope) and return its [`VariableInfo`].
    ///
    /// Returns `None` if the symbol is missing or isn't a variable.
    pub fn lookup_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a module static binding by name (in any scope) and return its [`StaticInfo`].
    pub fn lookup_static_info(&self, name: &str) -> Option<&StaticInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Static(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a variable by walking the whole enclosing scope chain, not just the innermost scope.
    ///
    /// This is what plain assignment resolves against. `scopes_and_name_resolution.md` states the contract
    /// directly: "if `x` already exists in any enclosing scope, it is a **reassignment** (so `x` must be mutable)",
    /// and only `let`/`mut` introduce a binding that may shadow an outer one.
    ///
    /// [`Self::lookup_local_variable_info`] answers a different question — "is this name bound *here*" — and is
    /// still the right lookup for checks that are genuinely about the innermost scope. Using it for assignment is
    /// what let a plain assignment inside any block silently create a fresh immutable binding instead of
    /// reassigning the outer one, with no diagnostic even when the types disagreed (#1072).
    pub fn lookup_variable_info_in_scope_chain(&self, name: &str) -> Option<&VariableInfo> {
        let id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a variable binding by name in the current scope only.
    ///
    /// Returns `None` if the symbol is missing, is only present in an enclosing scope, or is not a variable.
    pub fn lookup_local_variable_info(&self, name: &str) -> Option<&VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get(id)?;
        match &sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Look up a trait by name and return its [`TraitInfo`], if known.
    ///
    /// Returns `None` if the symbol is missing or isn't a trait.
    pub fn lookup_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        let sym = self.lookup_symbol(name)?;
        match &sym.kind {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }

    /// Look up semantic type metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This is intentionally narrower than [`Self::lookup_type_info`]: it is for internal semantic reasoning such as
    /// method lookup and trait compatibility, not for making unimported provider types available in user source.
    pub fn lookup_semantic_type_info(&self, name: &str) -> Option<&TypeInfo> {
        if let Some(info) = self.lookup_type_info(name) {
            return Some(info);
        }
        // A value may shadow the source spelling after an annotation has already selected a nominal type. Retain
        // that accepted type's exact artifact/declaration identity rather than resolving the value as a type again.
        if let Some(identity) = self.public_library_type_identities.get(name)
            && let Some(info) = self
                .public_library_type_identities
                .iter()
                .filter(|(binding, candidate)| {
                    split_canonical_public_library_type_name(binding).is_some() && *candidate == identity
                })
                .find_map(|(binding, _)| self.transitive_pub_types.get(binding).and_then(|infos| infos.first()))
        {
            return Some(info);
        }
        // A project's own transitively-reachable type wins over a stdlib stub of the same name. Both are fallbacks
        // for a name the current module does not declare, but only one of them is a type this program actually
        // built: a user class called `Registry` resolved to the primitive package's `Registry` and reported its own
        // `pub` field private, because the stub was consulted first.
        //
        // Requiring a single entry is what makes that safe for a bare name, where two dependencies may each export a
        // `Registry` and choosing one silently would resolve a user's type to a stranger's declaration. A
        // provider-qualified key cannot be ambiguous that way -- it already names one dependency and one export --
        // so repeated entries under it are the same declaration reached again, once per hop of the facade chain that
        // republishes it. Counting those repeats as ambiguity left a package model with no resolvable owner, so a
        // method it gets from an adopted trait resolved through no trait at all and emitted a plain Rust method call
        // that nothing in scope provides.
        if let Some(infos) = self.transitive_pub_types.get(name)
            && let Some(first) = infos.first()
            && (infos.len() == 1 || split_canonical_public_library_type_name(name).is_some())
        {
            return Some(first);
        }
        // Only a carrier key can equal a nominal's carrier key; any other name skips the symbol scan.
        if !name.starts_with(incan_semantics_core::body_ir::NOMINAL_CARRIER_KEY_PREFIX) {
            return None;
        }
        for (id, symbol) in self.symbols.all_symbols().iter().enumerate() {
            let SymbolKind::Type(info) = &symbol.kind else { continue };
            if symbol.scope == 0
                && self.symbols.is_active_lookup_binding(id)
                && self
                    .symbols
                    .identity_of(id)
                    .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
                    .as_deref()
                    == Some(name)
            {
                return Some(info);
            }
        }
        None
    }

    /// Look up semantic trait metadata, including transitive `pub::` exports referenced only through imported
    /// signatures.
    ///
    /// This keeps imported trait contracts available for internal compatibility checks without widening source-visible
    /// name resolution. A name the module binds to a declared type or a type parameter names that type: a stdlib or
    /// dependency trait that shares the name, and that the module did not import, is not what the name spells
    /// (`class Index` beside the stdlib's `Index` trait).
    pub fn lookup_semantic_trait_info(&self, name: &str) -> Option<&TraitInfo> {
        if let Some(symbol) = self.lookup_symbol(name) {
            match &symbol.kind {
                SymbolKind::Trait(info) => return Some(info),
                SymbolKind::Type(TypeInfo::Builtin) if symbol.scope == 0 => {}
                SymbolKind::Type(_) => return None,
                _ => {}
            }
        }
        let infos = self.transitive_pub_traits.get(name)?;
        (infos.len() == 1).then(|| &infos[0])
    }

    /// Look up the exact trait metadata named by a collected adoption.
    ///
    /// Dependency types retain the spelling used in their declaring module (for example `json.Serialize`) together
    /// with the provider module and source trait name. That qualified spelling is not a consumer binding, so exact
    /// provider metadata is consulted after ordinary semantic lookup rather than leaking the dependency's import into
    /// the consumer scope. It is consulted first when the spelling names another trait here (see
    /// [`Self::adoption_spelling_names_another_trait`]).
    pub fn lookup_trait_adoption_info(&self, adoption: &TypeBoundInfo) -> Option<&TraitInfo> {
        let exact = || {
            let module_path = adoption.module_path.as_ref()?;
            let source_name = adoption
                .source_name
                .as_deref()
                .or_else(|| adoption.name.rsplit('.').next())?;
            self.dependency_module_traits
                .get(&format!("{}.{}", module_path.join("."), source_name))
        };
        if self.adoption_spelling_names_another_trait(adoption)
            && let Some(info) = exact()
        {
            return Some(info);
        }
        self.lookup_semantic_trait_info(&adoption.name).or_else(exact)
    }

    /// Whether the bare name a standard-library trait adoption of another module's type spells names another trait,
    /// or none, in this module (#1561).
    ///
    /// An adoption keeps the spelling of the module that declares the adopting type. A type imported from another
    /// module that adopts `std.derives.comparison.Ord` spells `Ord` there, and in a module that does not import `Ord`
    /// that name is the builtin's methodless stub, so the defaults the type gets from the trait (`__ge__`, `__ne__`)
    /// were not found on it. A standard-library trait reaches a module through an import of the standard library, so a
    /// bare name that no such import binds does not name it here, except in the standard library's own source. The
    /// adopted trait's exact metadata is then read by its module path.
    pub(super) fn adoption_spelling_names_another_trait(&self, adoption: &TypeBoundInfo) -> bool {
        let under_stdlib = adoption
            .module_path
            .as_ref()
            .and_then(|path| path.first())
            .is_some_and(|root| root == stdlib::STDLIB_ROOT);
        if !under_stdlib || adoption.name.contains('.') || self.checks_standard_library_source() {
            return false;
        }
        !self
            .import_binding_path(&adoption.name)
            .and_then(|bound| bound.first())
            .is_some_and(|root| root == stdlib::STDLIB_ROOT)
    }

    /// The name a method dispatched through a collected adoption names the adopted trait by in this module (#1561).
    ///
    /// An adoption keeps the spelling of the module that declares the adopting type, which may be an import alias
    /// (`from std.derives.comparison import Ord as Ordered`, then `enum Level with Ordered`). Where that spelling names
    /// another trait, or none, here (see [`Self::adoption_spelling_names_another_trait`]), a call of the trait's method
    /// on the imported type names the trait by its declaration's own name beside its module path, as the recorded
    /// dispatch is read in this module: `Level.High.__gt__(Level.Low)` named `comparison::Ordered`, which no module of
    /// the standard library declares (E0433 against the compiled SDK).
    pub(in crate::typechecker) fn adoption_trait_name_here(&self, adoption: &TypeBoundInfo) -> String {
        match adoption.source_name.as_deref() {
            Some(declaration_name) if self.adoption_spelling_names_another_trait(adoption) => {
                declaration_name.to_string()
            }
            _ => adoption.name.clone(),
        }
    }

    /// Return the transitive supertrait closure for one trait using visible symbols first, then cached `pub::`
    /// semantic metadata.
    ///
    /// The returned `(trait_name, type_arguments)` pairs preserve generic substitution along the supertrait chain just
    /// like the local collection-phase closure used for in-module traits.
    pub fn semantic_supertrait_closure(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        if let Some(closure) = self.supertrait_closure.get(trait_name) {
            return closure.clone();
        }
        self.expand_semantic_supertraits_transitively(trait_name)
    }

    /// Expand transitive supertraits for imported trait metadata that never entered the local declaration collector.
    pub(super) fn expand_semantic_supertraits_transitively(
        &self,
        trait_name: &str,
    ) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut work = Vec::new();
        let Some(root) = self.lookup_semantic_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());

        while let Some((supertrait_name, supertrait_args)) = work.pop() {
            let key = format!(
                "{supertrait_name}<{}>",
                supertrait_args
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((supertrait_name.clone(), supertrait_args.clone()));
            let Some(supertrait_info) = self.lookup_semantic_trait_info(supertrait_name.as_str()) else {
                continue;
            };
            let subst =
                crate::resolved_type_subst::type_param_subst_map(&supertrait_info.type_params, &supertrait_args);
            for (nested_name, nested_args) in &supertrait_info.supertraits {
                let mapped = nested_args
                    .iter()
                    .map(|arg| crate::resolved_type_subst::substitute_resolved_type(arg, &subst))
                    .collect();
                work.push((nested_name.clone(), mapped));
            }
        }

        result
    }

    /// Mutable variant of [`lookup_local_variable_info`](Self::lookup_local_variable_info).
    ///
    /// Used by const-eval to update inferred types after evaluation.
    pub fn lookup_local_variable_info_mut(&mut self, name: &str) -> Option<&mut VariableInfo> {
        let id = self.symbols.lookup_local(name)?;
        let sym = self.symbols.get_mut(id)?;
        match &mut sym.kind {
            SymbolKind::Variable(info) => Some(info),
            _ => None,
        }
    }

    /// Resolve a required field type for the active trait default method, if any.
    ///
    /// Retain the declaring trait's member identity for the adopter's checked field binding. Emit a dedicated
    /// diagnostic if a trait body accesses an undeclared required field.
    pub fn trait_required_field_type(&mut self, field: &str, span: Span) -> Option<ResolvedType> {
        let requires = self.current_trait_requires.as_ref()?;
        if let Some(ty) = requires.get(field).cloned() {
            if let Some(owner) = self
                .current_trait_name
                .as_deref()
                .and_then(|name| self.symbols.lookup(name))
                .and_then(|symbol| self.symbols.identity_of(symbol))
            {
                let identity = Self::synthetic_member_identity(owner, field, SemanticSourceTargetKind::Field);
                self.type_info.record_resolved_identity(span, identity);
            }
            return Some(ty);
        }
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_ref()
            && seen.contains(field)
        {
            return None;
        }
        let trait_name = self.current_trait_name.as_deref().unwrap_or("<trait>");
        self.errors
            .push(errors::trait_requires_missing_field(trait_name, field, span));
        if let Some(seen) = self.current_trait_missing_requires_emitted.as_mut() {
            seen.insert(field.to_string());
        }
        None
    }
}

/// Whether an adoption a compiled package's type recorded names the package trait declared in `module_path` as `name`.
///
/// The caller has already proved that the bound and adopting type come from the same package. Compare their declaring
/// module and declaration name after normalizing the library entrypoint spelling used on each side of the manifest.
pub(super) fn adoption_names_declaration(adoption: &TypeBoundInfo, module_path: &[String], name: &str) -> bool {
    let trait_name = adoption
        .source_name
        .as_deref()
        .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
    adoption.module_path.as_deref().is_some_and(|adoption_module| {
        canonicalize_library_declaration_module_path(adoption_module)
            == canonicalize_library_declaration_module_path(module_path)
    }) && trait_name == name
}

/// Whether a trait adoption that records its trait's module names the trait a bound resolved to.
///
/// `bound` is the bound's `(module path, trait name)` in the checking module. The adoption's trait name is its source
/// name when it has one (`Serialize` for an adoption the declaring module spelled `json.Serialize`), so the comparison
/// does not depend on which names the checking module has imported. An adoption without a recorded module, or a bound
/// that does not resolve, answers `false` and is left to the name comparisons.
fn adoption_names_bound_identity(adoption: &TypeBoundInfo, bound: Option<&(Vec<String>, String)>) -> bool {
    let (Some(module_path), Some((bound_module, bound_trait))) = (adoption.module_path.as_ref(), bound) else {
        return false;
    };
    let trait_name = adoption
        .source_name
        .as_deref()
        .unwrap_or_else(|| adoption.name.rsplit('.').next().unwrap_or(adoption.name.as_str()));
    module_path == bound_module && trait_name == bound_trait
}
