//! Scopes and symbol definitions, including import, alias and registered bindings.

use super::*;

impl SymbolTable {
    /// Return the physical index that the next entered scope will receive. Retained checked lexical identities
    /// must match this locator before body reuse; this value never becomes an edit-stable semantic identity.
    pub(crate) fn next_scope_index(&self) -> usize {
        self.scopes.len()
    }

    /// Enter a new scope
    pub fn enter_scope(&mut self, kind: ScopeKind) {
        let new_scope = Scope::new(Some(self.current_scope), kind);
        self.scopes.push(new_scope);
        self.current_scope = self.scopes.len() - 1;
    }

    /// Exit the current scope, invalidating nominal facts when a module type's lexical shadow disappears.
    pub fn exit_scope(&mut self) {
        if let Some(parent) = self.scopes[self.current_scope].parent {
            self.invalidate_nominal_shadows();
            self.current_scope = parent;
        }
    }

    /// Leave the prepared lexical scope while retaining its exact bindings for a later statement producer. This
    /// invalidates nominal shadows through the same exit rule as ordinary lexical checking.
    pub(crate) fn suspend_scope(&mut self) -> PreparedScope {
        let prepared = PreparedScope(self.current_scope);
        self.exit_scope();
        prepared
    }

    /// Resume a prepared scope under the parent that created it. Re-entering a shadowing scope changes nominal
    /// visibility just as leaving it does, while every declaration identity remains unchanged.
    pub(crate) fn resume_scope(&mut self, prepared: PreparedScope) {
        debug_assert_eq!(self.scopes[prepared.0].parent, Some(self.current_scope));
        self.current_scope = prepared.0;
        self.invalidate_nominal_shadows();
    }

    /// Mark nominal visibility changed when entering or leaving a scope that shadows a module type. Scope transfer
    /// and ordinary lexical exit share this rule so a prepared frame cannot retain an obsolete nominal snapshot.
    pub(super) fn invalidate_nominal_shadows(&mut self) {
        if self.scopes[self.current_scope]
            .symbols
            .keys()
            .any(|name| self.nominal_binding_names.contains(name))
        {
            self.nominal_revision = InventoryRevision::default();
        }
    }

    /// Define a new symbol in the current scope through the shared RFC 120 registration mechanism.
    pub fn define(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a binding introduced through an explicit `let` or `mut` shadowing form.
    ///
    /// RFC 120 makes these forms declarations even when the spelling is already active in this scope. The new
    /// declaration replaces the active registration without producing a duplicate-binding diagnostic; later plain
    /// assignment and reference lookup therefore select the new binding and its fresh canonical identity.
    pub fn define_explicit_shadow(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::AllowExplicitShadow,
        )
    }

    /// Define a compiler-generated refinement of the currently-active binding.
    ///
    /// Branch narrowing changes the binding's type inside a nested scope but does not declare a new source object.
    /// The synthetic symbol therefore replaces the active registration while preserving the declaration identity it
    /// refines. An unresolved refinement retains the ordinary declaration-site fallback for recovery paths.
    pub fn define_refined_binding(&mut self, symbol: Symbol) -> SymbolId {
        let identity = self
            .lookup(&symbol.name)
            .and_then(|id| self.identities.get(&id))
            .cloned();
        let identity = identity.map_or(BindingIdentity::Declaration, |identity| {
            BindingIdentity::Target(Some(identity))
        });
        self.define_registered(symbol, identity, false, BindingDefinitionMode::AllowExplicitShadow)
    }

    /// Define a symbol whose identity kind the defining site states explicitly.
    ///
    /// The generic [`Self::define`] mint classifies a definition from its [`SymbolKind`], which cannot distinguish a
    /// parameter from a local (both are `Variable`) or a generic-binder placeholder from a builtin type (both are
    /// `Type(Builtin)`). Sites that know the RFC 120 declaration category pass it here so the minted identity records
    /// what was declared rather than how the table happens to represent it.
    pub fn define_with_target_kind(&mut self, symbol: Symbol, kind: SemanticSourceTargetKind) -> SymbolId {
        let identity = self.mint_identity_with_kind(&symbol, self.current_scope, kind);
        self.define_registered(
            symbol,
            BindingIdentity::Target(Some(identity)),
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define an import, alias, or re-export binding carrying its resolved target's identity.
    ///
    /// RFC 120: a binding created by an import is a binding to an *existing* canonical symbol — it must carry the
    /// declaring module's identity, never one minted from the importing module. `target_identity` is the identity
    /// import resolution proved (see `TypeChecker::dependency_member_identity`); `None` records the binding as
    /// unproven rather than inventing a consumer-module identity for it.
    pub fn define_import_binding(&mut self, symbol: Symbol, target_identity: Option<CanonicalSymbolId>) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            true,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a source alias carrying its target's identity without classifying the binding as an import.
    ///
    /// Aliases and imports both preserve a resolved declaration identity, but only syntactic imports participate in
    /// ambiguous-import diagnostics. An unresolved alias remains identity-less rather than minting an alias-site
    /// declaration identity.
    pub fn define_alias_binding(&mut self, symbol: Symbol, target_identity: Option<CanonicalSymbolId>) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            false,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define a checked source import and retain the exact path accepted for its local binding.
    ///
    /// The binding path is distinct from canonical declaration identity: an import may resolve through a facade while
    /// its identity names the module that owns the declaration. Decorator and trait resolution need the accepted
    /// source path, so they consume this sidecar rather than reconstructing it from raw AST imports.
    pub fn define_import_binding_at_path(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_import_binding(symbol, target_identity);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Define a source alias that preserves both its target identity and its checked import binding path.
    pub fn define_alias_binding_at_path(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_alias_binding(symbol, target_identity);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Define an import whose represented target kind is sufficient to mint its canonical identity.
    ///
    /// Rust item metadata carries the declaring crate path directly, so Rust imports use this path instead of asking
    /// their collector to duplicate the table's identity construction. Source imports continue to pass the identity
    /// proven by module resolution to [`Self::define_import_binding`]; module placeholders remain explicitly unproven.
    pub fn define_import_binding_with_inferred_target(&mut self, symbol: Symbol) -> SymbolId {
        let target_identity = self.mint_identity(&symbol);
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            true,
            BindingDefinitionMode::RejectCollision,
        )
    }

    /// Define an inferred-identity import reached through a checked source import path.
    pub fn define_import_binding_with_inferred_target_at_path(
        &mut self,
        symbol: Symbol,
        binding_path: Vec<String>,
    ) -> SymbolId {
        let id = self.define_import_binding_with_inferred_target(symbol);
        self.import_binding_paths.insert(id, binding_path);
        id
    }

    /// Remove a symbol's minted identity after its definition stops naming one declaration.
    ///
    /// The one current case is a module function symbol converted in place into an overload set: the identity minted
    /// for the first declaration would otherwise keep naming an arbitrary member of the set. Overload declarations
    /// keep their own span-keyed identities on `FunctionBindingInfo`.
    pub fn clear_identity(&mut self, id: SymbolId) {
        self.invalidate_nominal_symbol(id);
        self.identities.remove(&id);
    }

    /// Attach a later-proven import-resolution identity to the import binding it proves.
    ///
    /// Import materialization and identity proof do not always happen in one order, so a binding defined before its
    /// proof was recorded starts identity-less. This attaches the proof to the *current* binding for `name` only
    /// when that binding was defined as an import binding and still has no identity — a local declaration or
    /// overload set that has since shadowed the import keeps its own facts, and a binding that already carries an
    /// identity is never overwritten.
    pub fn backfill_import_identity(&mut self, name: &str, identity: &CanonicalSymbolId) {
        let Some(id) = self.lookup(name) else {
            return;
        };
        if self.import_bindings.contains(&id) && !self.identities.contains_key(&id) {
            self.invalidate_nominal_symbol(id);
            self.identities.insert(id, identity.clone());
        }
    }

    /// Return whether a retained symbol is the binding ordinary lookup currently selects.
    ///
    /// Rejected definitions remain in the arena for diagnostics and identity evidence. Side tables keyed only by a
    /// spelling must therefore commit their metadata only when this returns `true`; otherwise they would become
    /// last-wins while lexical lookup remains first-wins.
    pub fn is_active_lookup_binding(&self, id: SymbolId) -> bool {
        self.get(id).is_some_and(|symbol| self.lookup(&symbol.name) == Some(id))
    }

    /// Return the checked source import path associated with the active binding for `name`.
    pub fn import_binding_path(&self, name: &str) -> Option<&[String]> {
        let id = self.lookup(name)?;
        self.import_binding_paths.get(&id).map(Vec::as_slice)
    }

    /// Iterate checked source import paths for active module-scope bindings.
    pub fn active_import_binding_paths(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.scopes[0].symbols.iter().filter_map(|(name, id)| {
            self.import_binding_paths
                .get(id)
                .map(|path| (name.as_str(), path.as_slice()))
        })
    }

    /// Iterate active top-level source bindings in definition order for the HIR handoff.
    ///
    /// The symbol arena preserves definition order while the root lookup map does not. Filtering through that map
    /// keeps first-wins collision semantics: rejected later definitions remain available for diagnostics but do not
    /// become HIR bindings. Builtins and dependency-interface declarations have no consumer source declaration.
    pub fn active_module_source_bindings(&self) -> impl Iterator<Item = (Span, &str, Option<&CanonicalSymbolId>)> + '_ {
        self.symbols.iter().enumerate().filter_map(|(id, symbol)| {
            if symbol.scope != 0
                || symbol.span == Span::default()
                || self.dependency_interface_symbol_ids.contains(&id)
                || self.scopes[0].symbols.get(&symbol.name) != Some(&id)
            {
                return None;
            }
            Some((symbol.span, symbol.name.as_str(), self.identities.get(&id)))
        })
    }

    /// Define a symbol without replacing an existing same-scope lookup binding.
    ///
    /// Enum variants need to remain available to whole-table consumers such as match exhaustiveness and qualified
    /// pattern resolution, but a variant named like an imported type must not steal the bare identifier from that type.
    pub fn define_preserving_existing_binding(&mut self, symbol: Symbol) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Declaration,
            false,
            BindingDefinitionMode::PreserveExistingLookup,
        )
    }

    /// Define an alias that preserves both the target identity and any already-active bare lookup binding.
    pub fn define_alias_preserving_existing_binding(
        &mut self,
        symbol: Symbol,
        target_identity: Option<CanonicalSymbolId>,
    ) -> SymbolId {
        self.define_registered(
            symbol,
            BindingIdentity::Target(target_identity),
            false,
            BindingDefinitionMode::PreserveExistingLookup,
        )
    }

    /// Register and retain one symbol definition.
    ///
    /// `identity` decides whether this binding mints a declaration identity or preserves a resolved target identity. A
    /// missing target identity stays missing rather than becoming an alias-site declaration. Builtins occupy the
    /// fallback tier and deliberately bypass source-binding registration, so a source declaration such as `len` can
    /// replace that lookup spelling without a collision. `mode` records the source binding form: ordinary definitions
    /// reject collisions, explicit `let`/`mut` declarations replace an active registration, and member convenience
    /// bindings such as enum variants retain metadata without stealing an ordinary lexical lookup.
    pub(super) fn define_registered(
        &mut self,
        mut symbol: Symbol,
        identity: BindingIdentity,
        is_import: bool,
        mode: BindingDefinitionMode,
    ) -> SymbolId {
        self.record_current_scope_binding_before_change(&symbol.name);
        symbol.scope = self.current_scope;
        if symbol.scope == 0 && matches!(symbol.kind, SymbolKind::Type(_)) {
            self.nominal_binding_names.insert(symbol.name.clone());
        }
        let id = self.symbols.len();
        let identity = match identity {
            BindingIdentity::Declaration => self.mint_identity(&symbol),
            BindingIdentity::Target(identity) => identity,
        };
        if let Some(identity) = identity {
            self.identities.insert(id, identity);
        }
        if is_import {
            self.import_bindings.insert(id);
        }

        let name = symbol.name.clone();
        self.symbols.push(symbol);

        if self.minting_builtins {
            self.scopes[self.current_scope].symbols.insert(name, id);
            return id;
        }
        if let Some(bindings) = &mut self.dependency_interface_bindings {
            // The dependency view has no collision keys, so it must apply the member-binding rule by mode: a variant's
            // bare spelling defers to a lexical binding of the same name exactly as it does in a consumer scope.
            // Letting the variant overwrite the binding hid a dependency's derivable trait behind its own exported
            // enum variant, so the trait bound and the module derive resolved to nothing (#1429).
            if mode == BindingDefinitionMode::PreserveExistingLookup {
                bindings.entry(name).or_insert(id);
            } else {
                bindings.insert(name, id);
            }
            self.dependency_interface_symbol_ids.insert(id);
            return id;
        }

        let key = self.binding_key(id);
        if key.namespace == SymbolNamespace::OrdinaryLexical
            && let Some(existing) = self.immutable_builtin_binding(&name)
        {
            self.binding_keys.insert(id, key.clone());
            self.record_binding_collision(existing, id, &key, is_import);
            return id;
        }
        if mode == BindingDefinitionMode::AllowExplicitShadow {
            self.scopes[self.current_scope]
                .binding_registrations
                .insert(key.clone(), id);
            self.binding_keys.insert(id, key);
            self.scopes[self.current_scope].symbols.insert(name, id);
            return id;
        }
        let registration = register_binding(
            &mut self.scopes[self.current_scope].binding_registrations,
            key.clone(),
            id,
        );
        self.binding_keys.insert(id, key.clone());
        match registration {
            BindingRegistration::Registered => {
                if mode == BindingDefinitionMode::PreserveExistingLookup {
                    self.scopes[self.current_scope].symbols.entry(name).or_insert(id);
                } else {
                    self.scopes[self.current_scope].symbols.insert(name, id);
                }
            }
            BindingRegistration::Collision { existing } => {
                self.record_binding_collision(existing, id, &key, is_import);
                // The first registration remains the active lookup binding. The rejected symbol and its identity are
                // retained for complete diagnostics and recovery analysis, but invalid source cannot silently change
                // what later references mean.
            }
        }
        id
    }

    /// Build the collision key for one already-retained symbol.
    pub(super) fn binding_key(&self, id: SymbolId) -> BindingKey {
        let symbol = &self.symbols[id];
        let namespace = if self.import_bindings.contains(&id) {
            SymbolNamespace::OrdinaryLexical
        } else if let Some(identity) = self.identities.get(&id) {
            identity.namespace
        } else {
            default_identity_kind(&symbol.kind)
                .map(|kind| identity_namespace_for_kind(&kind))
                .unwrap_or(SymbolNamespace::OrdinaryLexical)
        };
        let owner = match &symbol.kind {
            SymbolKind::Variant(info) => Some(info.enum_name.clone()),
            _ => None,
        };
        BindingKey {
            namespace,
            owner,
            name: symbol.name.clone(),
        }
    }

    /// Return the root builtin binding when `name` is one of the immutable output-function spellings.
    ///
    /// Unlike ordinary builtins, this check is global across lexical scopes and applies before explicit-shadow mode:
    /// no ordinary lexical declaration or import binding may reuse `print` or `println`. Member names live in a
    /// separate namespace and do not replace either builtin.
    pub(super) fn immutable_builtin_binding(&self, name: &str) -> Option<SymbolId> {
        (builtins::from_str(name) == Some(BuiltinFnId::Print))
            .then(|| self.scopes[0].symbols.get(name).copied())
            .flatten()
    }

    /// Retain both sides of one rejected registration for typechecker-owned diagnostics.
    pub(super) fn record_binding_collision(
        &mut self,
        existing: SymbolId,
        incoming: SymbolId,
        key: &BindingKey,
        is_import: bool,
    ) {
        self.binding_collisions.push(SymbolBindingCollision {
            name: self.symbols[incoming].name.clone(),
            namespace: key.namespace,
            existing_span: self.symbols[existing].span,
            incoming_span: self.symbols[incoming].span,
            existing_identity: self.identities.get(&existing).cloned(),
            incoming_identity: self.identities.get(&incoming).cloned(),
            existing_is_import: self.import_bindings.contains(&existing),
            incoming_is_import: is_import,
        });
    }
}
