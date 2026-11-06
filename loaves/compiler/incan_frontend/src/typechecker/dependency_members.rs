//! Dependency member symbol caches, re-export walks, and member resolution.

use super::*;

impl TypeChecker {
    /// Cache exact member symbols exported by one dependency module for source `from ... import ...` resolution.
    pub(super) fn cache_dependency_member_symbols(
        &mut self,
        module_name: &str,
        module_ast: &Program,
        public_only: bool,
    ) {
        self.retain_body_ir_dependency_pub_reexports(module_name, module_ast, public_only);
        let mut member_symbols = HashMap::new();
        let mut member_type_aliases = HashMap::new();
        let mut member_projections = HashMap::new();
        if let Some(direct_symbols) = self.dependency_direct_member_symbols.get(module_name) {
            member_symbols.extend(direct_symbols.clone());
        } else {
            self.cache_dependency_direct_member_symbols(module_name, module_ast, public_only);
            if let Some(direct_symbols) = self.dependency_direct_member_symbols.get(module_name) {
                member_symbols.extend(direct_symbols.clone());
            }
        }
        if let Some(direct_projections) = self.dependency_direct_member_partial_projections.get(module_name) {
            member_projections.extend(direct_projections.clone());
        }
        if let Some(direct_aliases) = self.dependency_direct_member_type_aliases.get(module_name) {
            member_type_aliases.extend(direct_aliases.clone());
        }
        let mut reexports = HashMap::new();
        // A facade's own relative imports resolve against the facade's module, not the consumer's. Resolving them
        // from the consumer would bind `pkg.facade`'s `from helpers import render` to the root `helpers`.
        let facade_module_path = self
            .canonical_owner_segments(module_name)
            .unwrap_or_else(|| vec![module_name.to_string()]);
        for (exported_name, module, item_name) in Self::dependency_source_reexport_targets(module_ast) {
            reexports.insert(exported_name.clone(), (module.clone(), item_name.clone()));
            if let Some(kind) = self.dependency_reexported_member_symbol(&facade_module_path, &module, &item_name) {
                member_symbols.insert(exported_name.clone(), kind);
            }
            if let Some(target) =
                self.dependency_member_type_alias_for_path_from(&facade_module_path, &module, &item_name)
            {
                member_type_aliases.insert(exported_name.clone(), target);
            }
            if let Some(mut projection) =
                self.dependency_member_partial_projection_for_path_from(&facade_module_path, &module, &item_name)
            {
                projection.name.clone_from(&exported_name);
                member_projections.insert(exported_name, projection);
            }
        }
        for declaration in &module_ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            if public_only && import.visibility != Visibility::Public {
                continue;
            }
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            for item in items {
                let mut rust_path = vec![crate_name.clone()];
                rust_path.extend(path.iter().cloned());
                rust_path.push(item.name.clone());
                let canonical_path = rust_path.join("::");
                member_symbols.insert(
                    item.alias.as_ref().unwrap_or(&item.name).clone(),
                    SymbolKind::RustItem(RustItemInfo {
                        crate_name: crate_name.clone(),
                        path: canonical_path.clone(),
                        binding: RustImportBindingKind::FromImport,
                        metadata: self.rust_item_metadata_for_path(&canonical_path),
                    }),
                );
            }
        }
        self.dependency_member_reexports
            .insert(module_name.to_string(), reexports);
        self.dependency_member_symbols
            .insert(module_name.to_string(), member_symbols);
        self.dependency_member_type_aliases
            .insert(module_name.to_string(), member_type_aliases);
        self.dependency_member_partial_projections
            .insert(module_name.to_string(), member_projections);
    }

    /// Resolve a re-exported member from another source module or from the Loaf that owns the re-exported module.
    ///
    /// A facade that republishes `std.*` must bind the same declaration a direct `from std.x import f` binds: the
    /// owning Loaf's checked declaration, whose identity is what lets lowering recover the compiled signature,
    /// defaults included.
    ///
    /// Nominal types, constants and other admitted public members use this same route; limiting it to callables and
    /// traits leaves a nominal re-export as an identity-less placeholder at the next facade hop.
    ///
    /// A facade that writes `from std.serde.json import Serialize` re-exports that trait exactly as it re-exports a
    /// function, so a consumer importing the spelling from the facade must bind the stdlib trait rather than a module
    /// placeholder: the placeholder let adoption and method calls through unproven, and lowering then had only the
    /// spelling to key the trait's protocol on (#1431).
    pub(super) fn dependency_reexported_member_symbol(
        &mut self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<SymbolKind> {
        if let Some(kind) = self.dependency_member_symbol_for_path_from(base_module_path, module, item_name) {
            return Some(kind);
        }
        if module.parent_levels != 0 {
            return None;
        }
        let module_path = canonicalize_source_module_segments(&module.segments);
        let (library, rest) = self.loaf_module(&module_path)?;
        let (kind, _) = self.lookup_pub_library_module_symbol_member(&library, &rest, item_name)?;
        Some(kind)
    }

    /// The Loaf owning a bound module path and the public module path inside its manifest: `pub.<key>.<path>` for a
    /// dependency, `std.<root>.<path>` for a standard package other than the program's own.
    pub(in crate::typechecker) fn loaf_module(&self, module_path: &[String]) -> Option<(String, Vec<String>)> {
        if self.owns_standard_module(module_path) {
            return None;
        }
        let entry = self.loaves.owner_of(module_path)?;
        let root = entry.binding_root();
        Some((entry.key.clone(), module_path[root.len()..].to_vec()))
    }

    /// Return whether `module` is one of the program's own standard modules: the program is a standard package and
    /// the `std.*` root belongs to it, so the module is its own source rather than another Loaf.
    pub(crate) fn owns_standard_module(&self, module: &[String]) -> bool {
        match (self.own_standard_package, module) {
            (Some(policy), [root, name, ..]) => {
                root == incan_lang::lang::stdlib::STDLIB_ROOT && policy.namespace_roots.contains(&name.as_str())
            }
            _ => false,
        }
    }

    /// Cache dependency declarations in their declaring nominal scope before consumer imports expose them. Fields and
    /// callable signatures retain the same declaration owners as their carrier metadata.
    pub(super) fn cache_dependency_direct_member_symbols(
        &mut self,
        module_name: &str,
        module_ast: &Program,
        public_only: bool,
    ) {
        let mut direct_symbols = HashMap::new();
        let mut direct_type_aliases = HashMap::new();
        let mut direct_identities = HashMap::new();
        let mut direct_projections = HashMap::new();
        for name in Self::dependency_direct_member_names(module_ast, public_only) {
            if let Some(symbol_id) = self.symbols.lookup(name.as_str())
                && let Some(symbol) = self.symbols.get(symbol_id)
            {
                direct_symbols.insert(name.clone(), symbol.kind.clone());
                if let Some(identity) = self.symbols.identity_of(symbol_id) {
                    direct_identities.insert(name.clone(), identity.clone());
                }
            }
            if let Some(projection) = self.type_info.partial_projection(&name) {
                direct_projections.insert(name, projection.clone());
            }
        }
        let scope = self.checked_nominal_type_identities();
        let remapping = scope
            .iter()
            .filter_map(|(name, identity)| {
                incan_semantics_core::body_ir::nominal_carrier_key(identity).map(|key| (name.clone(), key))
            })
            .collect();
        let mut retained_scope = false;
        for (name, kind) in &mut direct_symbols {
            if let Some(identity) = direct_identities.get(name) {
                retained_scope |= self.retain_declaration_nominal_scope(identity, kind, &scope);
                self.remap_symbol_kind_with_import_aliases(kind, &remapping);
                if let SymbolKind::Type(info) = kind {
                    self.cache_retained_nominal_layout(Some(identity), info);
                }
            }
        }
        if retained_scope {
            self.retain_nominal_scope_carriers(&scope);
        }
        for declaration in &module_ast.declarations {
            let Declaration::TypeAlias(alias) = &declaration.node else {
                continue;
            };
            if public_only && alias.visibility != Visibility::Public {
                continue;
            }
            if let Some(target) = self.type_aliases.get(&alias.name) {
                let mut target = target.clone();
                let bindings = remapping
                    .iter()
                    .filter(|(name, _)| !target.type_params.contains(*name))
                    .map(|(name, qualified)| (name.clone(), qualified.clone()))
                    .collect();
                Self::remap_resolved_type_with_import_aliases(&mut target.target, &bindings);
                direct_type_aliases.insert(alias.name.clone(), target);
            }
        }
        for (name, kind) in &direct_symbols {
            match kind {
                SymbolKind::Type(info) if !matches!(info, TypeInfo::TypeAlias | TypeInfo::Builtin) => {
                    self.transitive_pub_types
                        .entry(name.clone())
                        .or_default()
                        .push(info.clone());
                }
                SymbolKind::Trait(info) => {
                    self.transitive_pub_traits
                        .entry(name.clone())
                        .or_default()
                        .push(info.clone());
                }
                _ => {}
            }
        }
        self.dependency_direct_member_symbols
            .insert(module_name.to_string(), direct_symbols);
        self.dependency_direct_member_type_aliases
            .insert(module_name.to_string(), direct_type_aliases);
        self.dependency_direct_member_identities
            .insert(module_name.to_string(), direct_identities);
        self.dependency_direct_member_partial_projections
            .insert(module_name.to_string(), direct_projections);
    }

    /// Share a module's immutable lexical scope across its declarations, fields and methods, returning whether this
    /// admission retained the supplied scope. A facade cannot replace an original or inherited member's scope.
    pub(super) fn retain_declaration_nominal_scope(
        &mut self,
        identity: &CanonicalSymbolId,
        kind: &SymbolKind,
        scope: &Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>,
    ) -> bool {
        let newly_retained = !self.declaration_nominal_scopes.contains_key(identity);
        let scope = self
            .declaration_nominal_scopes
            .entry(identity.clone())
            .or_insert_with(|| Arc::clone(scope));
        let scope = Arc::clone(scope);
        if let SymbolKind::Function(function) = kind {
            self.declaration_callables
                .entry(identity.clone())
                .or_insert_with(|| DeclarationCallableTypes {
                    params: function.params.clone(),
                    return_type: function.return_type.clone(),
                });
        }
        let fields = match kind {
            SymbolKind::Type(TypeInfo::Model(info)) => Some(&info.fields),
            SymbolKind::Type(TypeInfo::Class(info)) => Some(&info.fields),
            _ => None,
        };
        if let Some(fields) = fields {
            for member in fields.values().filter_map(|field| field.identity.as_ref()) {
                if member.origin == identity.origin {
                    self.declaration_nominal_scopes
                        .entry(member.clone())
                        .or_insert_with(|| Arc::clone(&scope));
                }
            }
        }
        let methods = match kind {
            SymbolKind::Type(TypeInfo::Model(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Class(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Newtype(info)) => &info.methods,
            SymbolKind::Type(TypeInfo::Enum(info)) => &info.methods,
            SymbolKind::Trait(info) => &info.methods,
            _ => return newly_retained,
        };
        for method in methods.values() {
            let Some(member) = &method.identity else { continue };
            if member.origin == identity.origin {
                self.declaration_nominal_scopes
                    .entry(member.clone())
                    .or_insert_with(|| Arc::clone(&scope));
                self.declaration_callables
                    .entry(member.clone())
                    .or_insert_with(|| DeclarationCallableTypes {
                        params: method.params.clone(),
                        return_type: method.return_type.clone(),
                    });
            }
        }
        newly_retained
    }

    /// Index a newly retained immutable scope once, retaining one spelling per canonical nominal. The scope registry
    /// never removes entries, so no discarded generation or source-scope snapshot is retained by this index.
    pub(super) fn retain_nominal_scope_carriers(
        &mut self,
        scope: &std::collections::BTreeMap<String, CanonicalSymbolId>,
    ) {
        for identity in scope.values() {
            if self.retained_nominal_carriers.contains_key(identity) {
                continue;
            }
            if let Some(key) = incan_semantics_core::body_ir::nominal_carrier_key(identity) {
                self.retained_nominal_carriers.insert(identity.clone(), key);
            }
        }
    }

    /// Return direct declaration names owned by a dependency module, excluding import re-exports.
    pub(super) fn dependency_direct_member_names(module_ast: &Program, public_only: bool) -> HashSet<String> {
        if public_only {
            let mut names = exported_symbols(module_ast)
                .into_iter()
                .filter_map(|symbol| match symbol {
                    ExportedSymbol::Const(name)
                    | ExportedSymbol::Static(name)
                    | ExportedSymbol::Type(name)
                    | ExportedSymbol::Trait(name)
                    | ExportedSymbol::Function(name)
                    | ExportedSymbol::Capability(name) => Some(name),
                    ExportedSymbol::Variant { variant_name, .. } => Some(variant_name),
                    ExportedSymbol::Reexported(_) => None,
                })
                .collect::<HashSet<_>>();
            names.extend(module_ast.declarations.iter().filter_map(|decl| {
                let Declaration::Alias(alias) = &decl.node else {
                    return None;
                };
                matches!(alias.visibility, Visibility::Public).then(|| alias.name.clone())
            }));
            return names;
        }

        module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Const(decl) => Some(decl.name.clone()),
                Declaration::Static(decl) => Some(decl.name.clone()),
                Declaration::Model(decl) => Some(decl.name.clone()),
                Declaration::Class(decl) => Some(decl.name.clone()),
                Declaration::Trait(decl) => Some(decl.name.clone()),
                Declaration::Alias(decl) => Some(decl.name.clone()),
                Declaration::Partial(decl) => Some(decl.name.clone()),
                Declaration::TypeAlias(decl) => Some(decl.name.clone()),
                Declaration::Newtype(decl) => Some(decl.name.clone()),
                Declaration::Enum(decl) => Some(decl.name.clone()),
                Declaration::Function(decl) => Some(decl.name.clone()),
                Declaration::Capability(decl) => Some(decl.name.clone()),
                Declaration::Import(_)
                | Declaration::Docstring(_)
                | Declaration::TestModule(_)
                | Declaration::VocabBlock(_) => None,
            })
            .collect()
    }

    /// Return source-module import targets that a dependency module makes visible as member bindings.
    pub(super) fn dependency_source_reexport_targets(module_ast: &Program) -> Vec<(String, ImportPath, String)> {
        module_ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Import(import) => Some(import),
                _ => None,
            })
            .filter_map(|import| match &import.kind {
                ImportKind::From { module, items } => Some((module.clone(), items.as_slice())),
                ImportKind::PubFrom { library, path, items } => {
                    let mut segments = vec![PUBLIC_LIBRARY_NAMESPACE.to_owned(), library.clone()];
                    segments.extend(path.iter().cloned());
                    Some((ImportPath::simple(segments), items.as_slice()))
                }
                _ => None,
            })
            .flat_map(|(module, items)| {
                items.iter().map(move |item| {
                    (
                        item.alias.as_ref().unwrap_or(&item.name).clone(),
                        module.clone(),
                        item.name.clone(),
                    )
                })
            })
            .collect()
    }

    /// Select one dependency cache entry using source-resolution order and an unambiguous nested-entry fallback.
    ///
    /// During SDK provider construction, a granted public `std.*` path resolves to the provider's physical source
    /// path before ordinary consumer candidates. This also applies while following facade re-exports: a facade such
    /// as physical `fs` may itself import `std.fs.file`, and that binding must retain the one `fs.file` declaration
    /// identity emitted by the same provider.
    pub(super) fn dependency_source_import_candidates(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
    ) -> Vec<Vec<String>> {
        let mut candidates = Vec::new();
        if module.parent_levels == 0 && self.owns_standard_module(&module.segments) {
            candidates.push(canonicalize_source_module_segments(&module.segments[1..]));
        }
        for candidate in logical_source_import_candidates(base_module_path, module) {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
        candidates
    }

    /// Select one dependency cache entry using source-resolution order and an unambiguous nested-entry fallback.
    pub(super) fn dependency_module_entry_for_path<'a, T>(
        &self,
        module: &ImportPath,
        entries: &'a HashMap<String, T>,
    ) -> Option<&'a T> {
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        self.dependency_module_entry_for_path_from(current_module_path, module, entries)
    }

    /// Select one dependency cache entry, resolving the import path against an explicit base module.
    ///
    /// A module's relative imports resolve against *its own* path. That is the consumer's path for an ordinary import,
    /// but a facade's own path when caching what that facade re-exports: `pkg.facade` writing `from helpers import
    /// render` means `pkg.helpers`, and resolving it from the consumer would silently bind the root `helpers` instead.
    pub(super) fn dependency_module_entry_for_path_from<'a, T>(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        entries: &'a HashMap<String, T>,
    ) -> Option<&'a T> {
        for candidate in self.dependency_source_import_candidates(base_module_path, module) {
            if let Some(entry) = entries.get(&candidate.join("_")) {
                return Some(entry);
            }
        }
        if module.is_absolute || module.parent_levels > 0 {
            return None;
        }

        // A nested CLI entrypoint is represented as logical `main`, so its sibling prefix is unavailable here even
        // though source discovery already admitted the exact dependency. Bind a bare suffix only when one cached
        // canonical module owns it; ambiguous same-leaf modules deliberately remain unresolved.
        let suffix = canonicalize_source_module_segments(&module.segments).join("_");
        if suffix.is_empty() {
            return None;
        }
        let suffix = format!("_{suffix}");
        let mut matches = entries
            .iter()
            .filter_map(|(key, entry)| key.ends_with(&suffix).then_some(entry));
        let entry = matches.next()?;
        matches.next().is_none().then_some(entry)
    }

    /// Return the exact symbol kind for one dependency module member path.
    pub fn dependency_member_symbol_for_path(&self, module: &ImportPath, item_name: &str) -> Option<SymbolKind> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_symbols)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency member's symbol kind, resolving the import path against an explicit base module.
    pub fn dependency_member_symbol_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<SymbolKind> {
        self.dependency_module_entry_for_path_from(base_module_path, module, &self.dependency_member_symbols)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency type alias target using ordinary consumer import resolution.
    pub fn dependency_member_type_alias_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<TypeAliasTarget> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_type_aliases)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency type alias target relative to a dependency module that is being collected or refreshed.
    pub(super) fn dependency_member_type_alias_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<TypeAliasTarget> {
        self.dependency_module_entry_for_path_from(base_module_path, module, &self.dependency_member_type_aliases)?
            .get(item_name)
            .cloned()
    }

    /// Maximum re-export hops followed while resolving one imported member to its declaring module.
    ///
    /// A malformed or cyclic facade chain must terminate rather than recurse forever; exceeding this yields no
    /// identity, which is the same fail-closed answer as any other unproven owner.
    pub(super) const MAX_REEXPORT_DEPTH: usize = 16;

    /// Register a dependency's real module path segments alongside the flattened name it is cached under.
    ///
    /// Caller-supplied metadata, not a derived cache: it deliberately survives `check_with_imports`, which clears the
    /// dependency caches it rebuilds. Registering before checking is therefore the expected order, and re-registering
    /// one name overwrites it.
    ///
    /// Optional: an unregistered dependency still resolves, and its identity falls back to the matching candidate's
    /// path. Registering is what makes the origin name the module that answered rather than the spelling that asked,
    /// which matters wherever the flattened name is ambiguous. Mirrors
    /// `IrCodegen::add_module_with_path_segments`, which carries the same pair for emission.
    pub fn register_dependency_module_path_segments(&mut self, module_name: &str, path_segments: Vec<String>) {
        if path_segments.is_empty() {
            return;
        }
        self.dependency_module_path_segments
            .insert(module_name.to_string(), path_segments);
    }

    /// Return the module path segments that canonically identify a cached dependency, if they are known.
    ///
    /// A declaration identity must never be built from a *projection* spelling. The cache key is the flattened,
    /// underscore-joined name that also names the emitted Rust module, and it is not injective: the path
    /// `pkg.helpers` and a module literally named `pkg_helpers` are one key. Reading segments back out of it would
    /// invent an origin for a module that may not exist, and would give one module two identities depending on which
    /// spelling reached it.
    ///
    /// The segments therefore come from an explicit registration, and otherwise only from the one case the flattened
    /// name determines by itself: a name containing no `_` can only be a single segment. Anything else is genuinely
    /// ambiguous and yields no identity rather than a guess.
    pub(super) fn canonical_owner_segments(&self, cache_key: &str) -> Option<Vec<String>> {
        if let Some(segments) = self.dependency_module_path_segments.get(cache_key) {
            return Some(segments.clone());
        }
        (!cache_key.is_empty() && !cache_key.contains('_')).then(|| vec![cache_key.to_string()])
    }

    /// Return the canonical identity of an imported member, following re-exports to the module that declares it.
    ///
    /// Import resolution is not literal: `logical_source_path_candidates` tries the sibling-relative path before the
    /// bare one, so the path written at an import is not necessarily the module it bound. An identity built from the
    /// written path can therefore name a different declaration that merely shares a leaf name. This follows the same
    /// candidate order resolution used, and where the winning candidate only re-exports the member it follows that
    /// link instead of naming the facade — a re-export is a binding to an existing declaration, not a second one.
    ///
    /// Returns `None` when the member is reachable only through an ambiguous fallback, when a re-export chain does not
    /// terminate in a declaration, or when the direct dependency cache lacks the compiler-minted canonical identity.
    /// A symbol kind, name, module key, and span are insufficient proof: absent canonical data must stay absent rather
    /// than being reconstructed into a plausible but invented declaration.
    pub fn dependency_member_identity(&self, module: &ImportPath, item_name: &str) -> Option<CanonicalSymbolId> {
        self.dependency_member_resolution(module, item_name)
            .map(|member| member.identity)
    }

    /// Resolve an imported member to its declaring identity and to the name its declaring module binds it under.
    ///
    /// The identity half is [`Self::dependency_member_identity`]. The name half is what a facade chain loses: a
    /// re-export may publish a declaration under a new name (`pub from provider import calculate as
    /// facade_calculate`), and a consumer of `facade_calculate` binds exactly the projection a direct import of
    /// `calculate` binds. The written item name therefore says nothing about how the declaring module spells the
    /// declaration, and lowering must not read a re-export rename as an `alias` declaration, which is a second name
    /// the declaring module itself binds to one identity. The walk knows the answer at its last hop: the item name it
    /// stopped at is the declaring module's own spelling, `calculate` for a re-export rename and `scale_alias` for
    /// `pub scale_alias = alias scale` (#1710).
    pub fn dependency_member_resolution(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<ResolvedDependencyMember> {
        // Resolve through the consumer's own module graph first. `dependency_source_import_candidates` already
        // orders a granted SDK provider ahead of ordinary candidates for a `std.*` path, so provider precedence is
        // preserved without keying on the spelling here.
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        if let Some(member) = self.dependency_member_resolution_from(current_module_path, module, item_name, 0) {
            return Some(member);
        }

        // Fall back to the provider registry keyed by the spelled path. A compiled provider is reachable by the path
        // its manifest publishes even when the consumer's source graph contains no module of that name, which is the
        // case this originally existed to serve.
        //
        // It must stay a fallback. Running it first let the spelling pre-empt resolution: `from helpers import
        // render` inside `pkg.app` selects the sibling `pkg.helpers`, but a root `helpers` declaring the same member
        // answered instead -- a module the import did not select.
        self.loaf_import_identity(module, item_name)
            .map(|identity| ResolvedDependencyMember::declared_as(identity, item_name))
    }

    /// Resolve one imported member to its declaring identity, using `from_module_path` as the resolution base.
    ///
    /// The base is the *consumer's* module for a direct import and the *facade's* module when following a re-export,
    /// because a facade's own relative import paths resolve against where the facade lives, not where the consumer
    /// does. Every arm that stops the walk answers by a direct lookup of `item_name` in the module it stopped at, so
    /// that name is the declaring module's spelling the result carries.
    pub(super) fn dependency_member_resolution_from(
        &self,
        from_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
        depth: usize,
    ) -> Option<ResolvedDependencyMember> {
        if depth >= Self::MAX_REEXPORT_DEPTH {
            return None;
        }
        for owner in self.dependency_source_import_candidates(from_module_path, module) {
            let key = owner.join("_");
            let resolves_here = self
                .dependency_member_symbols
                .get(&key)
                .is_some_and(|members| members.contains_key(item_name));
            if !resolves_here {
                continue;
            }

            // Resolution stops at this candidate. It owns the declaration only if it declares the member directly.
            if self
                .dependency_direct_member_symbols
                .get(&key)
                .and_then(|direct| direct.get(item_name))
                .is_some()
            {
                return self
                    .dependency_direct_member_identities
                    .get(&key)
                    .and_then(|identities| identities.get(item_name))
                    .cloned()
                    .map(|identity| ResolvedDependencyMember::declared_as(identity, item_name));
            }

            // Otherwise the member arrived here as a re-export. Follow it to the declaration it names, resolving
            // against the facade's own module — the same base `cache_dependency_member_symbols` used when it bound
            // that link, so the identity names exactly the declaration the binding selected. A link into `std.*`
            // reaches the same provider registry and source metadata the binding itself came from, in the same order.
            let (target_module, target_name) = self.dependency_member_reexports.get(&key)?.get(item_name)?;
            return self
                .dependency_member_resolution_from(&owner, target_module, target_name, depth + 1)
                .or_else(|| {
                    self.loaf_import_identity(target_module, target_name)
                        .map(|identity| ResolvedDependencyMember::declared_as(identity, target_name))
                });
        }
        None
    }

    /// Return the identity the Loaf owning `module` publishes for `item_name`: the identity half of
    /// [`Self::dependency_reexported_member_symbol`], followed hop for hop, so the identity a facade records is the
    /// identity of the symbol it bound.
    pub(super) fn loaf_import_identity(&self, module: &ImportPath, item_name: &str) -> Option<CanonicalSymbolId> {
        if module.parent_levels != 0 || module.is_absolute {
            return None;
        }
        self.loaf_member_identity(&canonicalize_source_module_segments(&module.segments), item_name)
    }

    /// Return the canonical identity the Loaf owning `module_path` publishes for `item_name`.
    pub(in crate::typechecker) fn loaf_member_identity(
        &self,
        module_path: &[String],
        item_name: &str,
    ) -> Option<CanonicalSymbolId> {
        let (library, rest) = self.loaf_module(module_path)?;
        let manifest = self.loaves.manifest(&library)?;
        let public_path = std::iter::once(manifest.name.clone())
            .chain(rest)
            .chain(std::iter::once(item_name.to_string()))
            .collect::<Vec<_>>();
        manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_path(&public_path)
    }
}
