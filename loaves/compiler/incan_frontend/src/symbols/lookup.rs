//! Canonical identity minting, declaration identities, lookups, binding transactions and return types.

use super::*;

impl SymbolTable {
    // ---- RFC 120 canonical identity minting ----

    /// Mint the canonical identity for one definition, when its symbol kind classifies it.
    ///
    /// Returns `None` while builtins are being registered (their loop attaches explicit registry identities so alias
    /// spellings share the canonical entry), for overload sets (each overload declaration owns a span-keyed identity
    /// of its own; a set-level identity would name an arbitrary member), and for definitions whose kind is a
    /// placeholder the site must classify via [`Self::define_with_target_kind`].
    pub(super) fn mint_identity(&self, symbol: &Symbol) -> Option<CanonicalSymbolId> {
        if self.minting_builtins {
            return None;
        }
        let kind = default_identity_kind(&symbol.kind)?;
        Some(self.mint_identity_with_kind(symbol, symbol.scope, kind))
    }

    /// Build one canonical identity for a definition with an already-decided declaration category.
    ///
    /// A `rust::` item's identity is anchored to its crate path with a zero declaration span: it has no Incan
    /// declaration site, and the zero span keeps every module's import of one item structurally equal instead of
    /// splitting per import site. Everything else is owned by the current module and anchored at its declaration
    /// span, with the defining scope as discriminant for bindings that are not module-unique.
    pub(super) fn mint_identity_with_kind(
        &self,
        symbol: &Symbol,
        scope: usize,
        kind: SemanticSourceTargetKind,
    ) -> CanonicalSymbolId {
        let mut kind = kind;
        let mut namespace = identity_namespace_for_kind(&kind);
        let (origin, declaration_name, declaration_span) = match &symbol.kind {
            SymbolKind::RustItem(info) => {
                if info.binding != RustImportBindingKind::FromImport {
                    namespace = SymbolNamespace::ModulePath;
                    kind = SemanticSourceTargetKind::Module;
                }
                (
                    SymbolOrigin::RustCrate(info.path.split("::").map(str::to_string).collect()),
                    info.path.rsplit("::").next().unwrap_or(&symbol.name).to_string(),
                    HirSourceSpan::new(0, 0),
                )
            }
            _ => (self.declaration_origin(), symbol.name.clone(), symbol.span),
        };
        // A member is owned by its nominal type and distinguished by its declaration span, not by which table scope
        // its convenience binding happened to be defined in — so member identities never carry a scope discriminant
        // and compare equal however they are reached.
        let scope_discriminant =
            (namespace != SymbolNamespace::Member && scope != 0).then_some(ScopeDiscriminant(scope));
        CanonicalSymbolId {
            namespace,
            origin,
            declaration_name,
            kind,
            scope_discriminant,
            declaration_span,
        }
    }

    /// Iterate the identities of module-scope declarations this module owns, paired with their declaration spans.
    ///
    /// Yields only definitions whose identity origin is the current module and whose declaration span is real:
    /// builtins carry the registry origin and zero spans, and cross-module import bindings carry the declaring
    /// module's origin, so neither can masquerade as a local declaration. A binding to a *same-module* declaration
    /// (an `alias` targeting a local symbol) does pass the filter and yields its binding site paired with the
    /// target declaration's identity — which is the RFC 120 answer for that site: the alias is a second binding to
    /// the existing declaration, not a second declaration.
    pub fn local_declaration_identities(&self) -> impl Iterator<Item = (Span, &CanonicalSymbolId)> + '_ {
        self.symbols.iter().enumerate().filter_map(|(id, symbol)| {
            if symbol.scope != 0 || symbol.span == Span::default() || self.dependency_interface_symbol_ids.contains(&id)
            {
                return None;
            }
            let identity = self.identities.get(&id)?;
            let owned_here = match (&identity.origin, self.package_identity.as_deref()) {
                (SymbolOrigin::Module(path), None) => path == &self.module_path,
                (SymbolOrigin::Package { library, module_path }, Some(package)) => {
                    library == package && module_path == &self.module_path
                }
                _ => false,
            };
            if !owned_here {
                return None;
            }
            Some((symbol.span, identity))
        })
    }

    /// Iterate source-owned nested declarations with their compiler-minted canonical identities.
    ///
    /// Unlike [`Self::local_declaration_identities`], this inventory includes declarations whose lexical scopes have
    /// already closed. Requiring the symbol site to equal the identity's declaration site excludes aliases and
    /// compiler-generated refinements, which carry another declaration's identity at a different binding site.
    pub fn nested_declaration_identities(&self) -> impl Iterator<Item = (Span, &CanonicalSymbolId)> + '_ {
        self.nested_declaration_identities_from(0)
    }

    /// Export only lexical declarations registered since a body scope's symbol cursor. The cursor is invocation-local,
    /// never a semantic identity; the same ownership/origin rules govern full and local export.
    pub(crate) fn nested_declaration_identities_from(
        &self,
        start: usize,
    ) -> impl Iterator<Item = (Span, &CanonicalSymbolId)> + '_ {
        self.symbols.iter().enumerate().skip(start).filter_map(|(id, symbol)| {
            if symbol.scope == 0 || symbol.span == Span::default() || self.dependency_interface_symbol_ids.contains(&id)
            {
                return None;
            }
            let identity = self.identities.get(&id)?;
            let identity_span = symbol.span;
            if identity.declaration_span != identity_span {
                return None;
            }
            let owned_here = match (&identity.origin, self.package_identity.as_deref()) {
                (SymbolOrigin::Module(path), None) => path == &self.module_path,
                (SymbolOrigin::Package { library, module_path }, Some(package)) => {
                    library == package && module_path == &self.module_path
                }
                _ => false,
            };
            owned_here.then_some((symbol.span, identity))
        })
    }

    /// Mint the canonical identity of one module-level declaration owned by the current module.
    ///
    /// The table is the single minting authority for RFC 120 identities, so sites that record declaration facts
    /// outside the symbol table (function/partial bindings, method bindings) obtain their identity here rather than
    /// assembling one from module path plus spelling themselves.
    pub fn module_declaration_identity(
        &self,
        name: &str,
        kind: SemanticSourceTargetKind,
        span: Span,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: identity_namespace_for_kind(&kind),
            origin: self.declaration_origin(),
            declaration_name: name.to_string(),
            kind,
            scope_discriminant: None,
            declaration_span: span,
        }
    }

    /// Build the canonical identity of one compiler-proven source or package module path.
    ///
    /// Module namespaces have no declaration token in the importing file, so their provenance anchor is zero. Their
    /// structural owner is the checked module graph path: project and `std` modules retain a module origin, while
    /// `pub::` paths retain their package owner. Rust namespace imports use the dedicated Rust branch in the mint.
    pub fn module_path_identity(path: &[String]) -> Option<CanonicalSymbolId> {
        let declaration_name = path.last()?.clone();
        let origin = match path {
            [root, library, rest @ ..] if root == "pub" => SymbolOrigin::Package {
                library: library.clone(),
                module_path: rest.to_vec(),
            },
            _ => SymbolOrigin::Module(path.to_vec()),
        };
        Some(CanonicalSymbolId {
            namespace: SymbolNamespace::ModulePath,
            origin,
            declaration_name,
            kind: SemanticSourceTargetKind::Module,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(0, 0),
        })
    }

    /// Mint the canonical identity of one member declaration owned by a nominal type in the current module.
    ///
    /// Members live in RFC 120's member namespace: they are reached `.`-directed from an owner type, never through
    /// the scope chain, so a member and a lexical binding sharing one spelling never compare equal. Two owners'
    /// same-named members stay distinct through their declaration spans.
    pub fn member_declaration_identity(
        &self,
        name: &str,
        kind: SemanticSourceTargetKind,
        span: Span,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::Member,
            origin: self.declaration_origin(),
            declaration_name: name.to_string(),
            kind,
            scope_discriminant: None,
            declaration_span: span,
        }
    }

    /// Attach the explicit registry identity for one builtin definition.
    ///
    /// Builtin alias spellings (`i64` for `int`, `println` for `print`) are separate table entries, but RFC 120's
    /// builtin fallback tier has one canonical entry per builtin — so every alias records the canonical registry
    /// spelling as its declaration name and all spellings compare equal.
    pub(super) fn record_builtin_identity(&mut self, id: SymbolId, canonical_name: &str) {
        self.invalidate_nominal_symbol(id);
        self.identities.insert(id, canonical_builtin_identity(canonical_name));
    }

    /// Look up a symbol by name in the current scope chain
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        let mut scope_idx = self.current_scope;
        loop {
            if scope_idx == 0
                && let Some(id) = self
                    .dependency_interface_bindings
                    .as_ref()
                    .and_then(|bindings| bindings.get(name))
            {
                return Some(*id);
            }
            if let Some(&id) = self.scopes[scope_idx].symbols.get(name) {
                return Some(id);
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Query the accepted module type binding of a resolved nominal type, independently of local value shadowing.
    ///
    /// Nominal declarations are module declarations, not statement-local declarations. A checked ResolvedType
    /// retains that binding's key; looking through the current value scope could instead select a parameter or local.
    pub fn module_type_identity(&self, name: &str) -> Option<&CanonicalSymbolId> {
        let id = self
            .dependency_interface_bindings
            .as_ref()
            .and_then(|bindings| bindings.get(name))
            .or_else(|| self.scopes[0].symbols.get(name))?;
        let symbol = self.get(*id)?;
        matches!(symbol.kind, SymbolKind::Type(_) | SymbolKind::Trait(_))
            .then(|| self.identities.get(id))
            .flatten()
    }

    /// Look up a symbol only in the current scope (no parent lookup)
    pub fn lookup_local(&self, name: &str) -> Option<SymbolId> {
        if self.current_scope == 0
            && let Some(id) = self
                .dependency_interface_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(name))
        {
            return Some(*id);
        }
        self.scopes[self.current_scope].symbols.get(name).copied()
    }

    /// Drain symbol-definition collisions for typechecker-owned diagnostics.
    pub fn take_binding_collisions(&mut self) -> Vec<SymbolBindingCollision> {
        std::mem::take(&mut self.binding_collisions)
    }

    /// Open a fresh temporary lookup view for one dependency interface.
    pub fn begin_dependency_interface_bindings(&mut self) {
        self.nominal_revision = InventoryRevision::default();
        debug_assert!(self.dependency_interface_bindings.is_none());
        self.dependency_interface_bindings = Some(HashMap::new());
    }

    /// Drop the active dependency-interface view before another module or consumer source is checked.
    pub fn finish_dependency_interface_bindings(&mut self) {
        self.nominal_revision = InventoryRevision::default();
        debug_assert!(self.dependency_interface_bindings.is_some());
        self.dependency_interface_bindings = None;
    }

    /// Start recording the previous value of each current-scope binding changed by subsequent definitions.
    pub fn begin_current_scope_binding_transaction(&mut self) {
        debug_assert!(self.current_scope_binding_transaction.is_none());
        self.current_scope_binding_transaction = Some(BindingTransaction {
            previous: HashMap::new(),
            collision_len: self.binding_collisions.len(),
        });
    }

    /// Finish the active binding transaction and return only the names it touched.
    pub fn finish_current_scope_binding_transaction(&mut self) -> HashMap<String, Option<SymbolId>> {
        let Some(transaction) = self.current_scope_binding_transaction.take() else {
            return HashMap::new();
        };
        // Dependency-import transactions temporarily materialize bindings only to collect checked public metadata.
        // Their collisions are not collisions in the consumer's lexical scope and must not leak into its diagnostics.
        self.binding_collisions.truncate(transaction.collision_len);
        transaction.previous
    }

    /// Record one binding before its first change in the active transaction.
    pub(super) fn record_current_scope_binding_before_change(&mut self, name: &str) {
        self.invalidate_nominal_binding(name);
        let previous = if self.current_scope == 0 {
            self.dependency_interface_bindings
                .as_ref()
                .and_then(|bindings| bindings.get(name).copied())
                .or_else(|| self.scopes[self.current_scope].symbols.get(name).copied())
        } else {
            self.scopes[self.current_scope].symbols.get(name).copied()
        };
        if let Some(transaction) = &mut self.current_scope_binding_transaction {
            transaction.previous.entry(name.to_string()).or_insert(previous);
        }
    }

    /// Restore or remove one name binding in the current scope without deleting historical symbol metadata.
    pub fn restore_current_scope_binding(&mut self, name: &str, binding: Option<SymbolId>) {
        self.invalidate_nominal_binding(name);
        if self.current_scope == 0
            && let Some(bindings) = &mut self.dependency_interface_bindings
        {
            if let Some(symbol_id) = binding {
                bindings.insert(name.to_string(), symbol_id);
            } else {
                bindings.remove(name);
            }
            return;
        }
        if let Some(current) = self.scopes[self.current_scope].symbols.get(name).copied()
            && let Some(key) = self.binding_keys.get(&current)
            && self.scopes[self.current_scope].binding_registrations.get(key) == Some(&current)
        {
            self.scopes[self.current_scope].binding_registrations.remove(key);
        }
        if let Some(symbol_id) = binding {
            self.scopes[self.current_scope]
                .symbols
                .insert(name.to_string(), symbol_id);
            if let Some(key) = self.binding_keys.get(&symbol_id).cloned() {
                self.scopes[self.current_scope]
                    .binding_registrations
                    .insert(key, symbol_id);
            }
        } else {
            self.scopes[self.current_scope].symbols.remove(name);
        }
    }

    /// Get a symbol by ID
    pub fn get(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id)
    }

    /// Get a mutable symbol by ID, invalidating nominal facts before root metadata can change.
    pub fn get_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.invalidate_nominal_symbol(id);
        self.symbols.get_mut(id)
    }

    /// Invalidate before mutable root-symbol access can alter its identity, kind or spelling.
    pub(super) fn invalidate_nominal_symbol(&mut self, id: SymbolId) {
        if let Some(symbol) = self.symbols.get(id)
            && symbol.scope == 0
        {
            self.nominal_binding_names.insert(symbol.name.clone());
            self.nominal_revision = InventoryRevision::default();
        }
    }

    /// Invalidate a changed root binding or a lexical shadow of a module type without recording another transaction.
    pub(super) fn invalidate_nominal_binding(&mut self, name: &str) {
        if self.current_scope == 0 || self.nominal_binding_names.contains(name) {
            self.nominal_revision = InventoryRevision::default();
        }
    }

    /// Borrow the mutation owner's witness without rescanning historical symbols or active alias targets.
    pub(crate) fn nominal_revision(&self) -> InventoryRevision {
        self.nominal_revision.clone()
    }

    /// Read the root lookup indexes once, omitting rejected definitions and bindings shadowed in the current scope.
    pub(crate) fn active_module_symbols(&self) -> impl Iterator<Item = (SymbolId, &Symbol)> {
        let imported = self.dependency_interface_bindings.as_ref();
        self.scopes[0]
            .symbols
            .iter()
            .filter(move |(name, _)| !imported.is_some_and(|bindings| bindings.contains_key(*name)))
            .chain(imported.into_iter().flat_map(|bindings| bindings.iter()))
            .filter_map(|(name, id)| {
                let symbol = self.symbols.get(*id)?;
                (symbol.scope == 0 && self.lookup(name) == Some(*id)).then_some((*id, symbol))
            })
    }

    /// All symbols in definition order (builtins first, then user declarations).
    ///
    /// Used for whole-program analyses such as supertrait graphs.
    pub fn all_symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    /// Return the index of the current scope; a scope entered later always has a larger index.
    pub fn current_scope_index(&self) -> usize {
        self.current_scope
    }

    /// Whether a read from the current scope of a binding held by the scope `defining` crosses a callable's own scope
    /// on the way out, so the read sits in a closure nested inside the callable that holds the binding and captures it.
    pub fn read_crosses_callable_scope(&self, defining: usize) -> bool {
        let mut scope_idx = self.current_scope;
        while scope_idx != defining {
            let Some(scope) = self.scopes.get(scope_idx) else {
                return false;
            };
            if matches!(scope.kind, ScopeKind::Function | ScopeKind::Method { .. }) {
                return true;
            }
            let Some(parent) = scope.parent else {
                return false;
            };
            scope_idx = parent;
        }
        false
    }

    /// Get the current function's return type (if in a function)
    pub fn current_return_type(&self) -> Option<&ResolvedType> {
        let mut scope_idx = self.current_scope;
        loop {
            match &self.scopes[scope_idx].kind {
                ScopeKind::Function | ScopeKind::Method { .. } => {
                    return self.scopes[scope_idx].return_type.as_ref();
                }
                _ => {}
            }
            if let Some(parent) = self.scopes[scope_idx].parent {
                scope_idx = parent;
            } else {
                break;
            }
        }
        None
    }

    /// Return the declared return type of the innermost enclosing function or method that declares one.
    ///
    /// A closure's scope declares no return type, so this looks past closures to the declaration whose body holds
    /// them, where [`Self::current_return_type`] stops at the closure.
    pub fn enclosing_declared_return_type(&self) -> Option<&ResolvedType> {
        let mut scope_idx = self.current_scope;
        loop {
            let scope = &self.scopes[scope_idx];
            if matches!(scope.kind, ScopeKind::Function | ScopeKind::Method { .. })
                && let Some(return_type) = scope.return_type.as_ref()
            {
                return Some(return_type);
            }
            scope_idx = scope.parent?;
        }
    }

    /// Set the return type for the current function scope
    pub fn set_return_type(&mut self, ty: ResolvedType) {
        self.scopes[self.current_scope].return_type = Some(ty);
    }
}
