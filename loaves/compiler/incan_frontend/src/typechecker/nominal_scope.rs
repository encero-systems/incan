//! Checked Rust owner facts and the published nominal scope.

use super::*;

impl TypeChecker {
    /// Retain checked native default templates at the checker boundary. Only successfully demanded type metadata
    /// contributes facts; Body IR never starts a second metadata query to choose an owner's argument count.
    pub(super) fn checked_rust_owner_parameters(&self) -> HashMap<String, Vec<(String, Option<ResolvedType>)>> {
        let mut owners = HashMap::new();
        for path in self.observed_rust_metadata_queries() {
            let Some(metadata) = self.lookup_rust_item_metadata_for_path(&path) else {
                continue;
            };
            let RustItemKind::Type(info) = &metadata.kind else {
                continue;
            };
            let parameters = info
                .type_params
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let default = info.type_param_defaults.get(index).and_then(Option::as_deref);
                    (
                        name.clone(),
                        default.map(|default| self.native_rust_signature_type(default, &path)),
                    )
                })
                .collect::<Vec<_>>();
            let public = self
                .type_info
                .rust
                .item_public_paths
                .get(&path)
                .cloned()
                .unwrap_or(path);
            owners.insert(public, parameters);
        }
        owners
    }

    /// Select the metadata producer's public owner when it belongs to the selected extern namespace. A private
    /// definition or a definition reached through another crate still needs the admitted public import projection.
    /// Source imports cannot choose a different carrier merely by naming another reexport of the same definition.
    pub(super) fn rust_public_definition_path<'a>(
        &self,
        path: &str,
        metadata: &'a RustItemMetadata,
    ) -> Option<&'a str> {
        let definition = metadata.definition_path.as_deref().unwrap_or(&metadata.canonical_path);
        if definition.split("::").next() != path.split("::").next() {
            return None;
        }
        let owner = self.rust_item_metadata_for_path(definition)?;
        (owner.visibility == incan_lang::interop::RustVisibility::Public).then_some(definition)
    }

    /// Map imported Rust spellings to the producer-selected public carrier per native definition and bound extern.
    /// The producer's DefId-backed path is independent of the importing module. Only inaccessible or cross-extern
    /// definitions use the smallest admitted public projection; private paths never become executable identities.
    pub(super) fn checked_rust_item_public_paths(&self) -> HashMap<String, String> {
        // ---- Public representatives of native definitions ----
        let mut type_paths: HashMap<(String, String), (String, bool)> = HashMap::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            let metadata = info
                .metadata
                .clone()
                .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
            let Some(metadata) = metadata else {
                continue;
            };
            if !matches!(metadata.kind, RustItemKind::Type(_)) {
                continue;
            }
            let definition = metadata.definition_path.as_ref().unwrap_or(&metadata.canonical_path);
            let imported = Self::normalize_rust_namespace_path(&info.path);
            let domain = imported.split("::").next().unwrap_or("").to_owned();
            match type_paths.entry((domain, definition.clone())) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let producer = self.rust_public_definition_path(imported, &metadata);
                    entry.insert((producer.unwrap_or(imported).to_owned(), producer.is_some()));
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let (selected, producer_selected) = entry.get_mut();
                    if !*producer_selected && imported < selected.as_str() {
                        *selected = imported.to_owned();
                    }
                }
            }
        }

        // ---- Source spellings and defining paths ----
        let mut paths = HashMap::new();
        for symbol in self.symbols.all_symbols() {
            let SymbolKind::RustItem(info) = &symbol.kind else {
                continue;
            };
            let public = Self::normalize_rust_namespace_path(info.path.as_str()).to_string();
            let metadata = info
                .metadata
                .clone()
                .or_else(|| self.rust_item_metadata_for_path(info.path.as_str()));
            let domain = public.split("::").next().unwrap_or("").to_owned();
            let canonical = metadata
                .as_ref()
                .filter(|metadata| matches!(metadata.kind, RustItemKind::Type(_)))
                .map(|metadata| metadata.definition_path.as_ref().unwrap_or(&metadata.canonical_path))
                .and_then(|definition| type_paths.get(&(domain, definition.clone())))
                .map(|(public, _)| public.clone())
                .unwrap_or_else(|| public.clone());
            if let Some(definition) = metadata.and_then(|metadata| metadata.definition_path.clone()) {
                paths.entry(definition).or_insert_with(|| canonical.clone());
            }
            if self.lookup_symbol(&symbol.name).is_some_and(
                |selected| matches!(&selected.kind, SymbolKind::RustItem(selected) if selected.path == info.path),
            ) {
                paths.insert(symbol.name.clone(), canonical.clone());
            }
            paths.insert(public, canonical);
        }
        paths
    }

    /// The `std.*` path of the module declaring `identity`, when a standard package declares it: another Loaf's
    /// declaration (its origin names the package) or one of the program's own standard modules.
    pub(in crate::typechecker) fn standard_declaring_module(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Option<Vec<String>> {
        let std_path = |path: &[String]| {
            std::iter::once(stdlib::STDLIB_ROOT.to_string())
                .chain(path.iter().cloned())
                .collect()
        };
        match &identity.origin {
            SymbolOrigin::Package { library, module_path }
                if incan_lang::lang::standard_packages::standard_package_namespace_policy(library).is_some() =>
            {
                Some(std_path(module_path))
            }
            SymbolOrigin::Module(path) if path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) => {
                Some(path.clone())
            }
            SymbolOrigin::Module(path) if self.own_standard_package.is_some() => Some(std_path(path)),
            _ => None,
        }
    }

    /// The identity of the import binding that spells the surface type `surface` (`Html`) in this module: the
    /// declaration the owning standard package publishes.
    pub(in crate::typechecker) fn surface_type_identity(&self, surface: SurfaceTypeId) -> Option<CanonicalSymbolId> {
        self.surface_type_import_bindings
            .values()
            .find(|(bound, symbol)| *bound == surface && self.symbols.is_active_lookup_binding(*symbol))
            .and_then(|(_, symbol)| self.symbols.identity_of(*symbol).cloned())
    }

    /// Retain active native builtin owners once for opaque nominal storage and checked runtime operations, without
    /// changing legacy lookup. Active rusttype aliases retain their checked source owner for native enum contracts.
    /// Web placeholders use their checked surface binding and original Rust re-export;
    /// the kernel whitelist validates identities and source shadows yield nothing.
    pub(super) fn checked_native_nominal_type_identities(
        &mut self,
    ) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self
            .symbols
            .active_module_symbols()
            .filter_map(|(id, symbol)| {
                let identity = self.symbols.identity_of(id)?;
                if identity.kind == SemanticSourceTargetKind::Rusttype {
                    return Some((symbol.name.clone(), identity.clone()));
                }
                if identity.origin != incan_semantics_core::SymbolOrigin::Builtin {
                    return None;
                }
                incan_semantics_core::body_ir::native_carrier_nominal(identity)?;
                Some((symbol.name.clone(), identity.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        for (name, (_, symbol)) in &self.surface_type_import_bindings {
            if self.symbols.lookup(name) == Some(*symbol)
                && let Some(identity) = self.symbols.identity_of(*symbol)
            {
                identities.insert(name.clone(), identity.clone());
            }
        }
        // Generated reflection calls retain their published runtime carrier without introducing a source binding.
        for (name, identity) in &self.type_info.declarations.body_ir_native_type_identities {
            if matches!(identity.origin, SymbolOrigin::Package { .. })
                && incan_semantics_core::body_ir::native_nominal_item(identity).is_some()
            {
                identities.insert(name.clone(), identity.clone());
            }
        }
        if let Some(identity) = self.runtime_surface_native_identity(SurfaceTypeId::TaskJoinError) {
            identities.insert(surface_types::TASK_JOIN_ERROR_TYPE_NAME.to_owned(), identity);
        }
        identities
    }

    /// Return one runtime-owned surface type's native identity after authenticating its exact package declaration.
    /// Inferred carriers use the backing Rust item because a transparent `rusttype` authors no executable Loaf body.
    fn runtime_surface_native_identity(&self, surface: SurfaceTypeId) -> Option<CanonicalSymbolId> {
        let surface_module = surface_types::stdlib_module_path(surface)?;
        let namespace = surface_module.split('.').nth(1)?;
        let package = incan_lang::lang::stdlib::find_namespace(namespace)?.package?;
        let manifest = self.loaves.manifest(package)?;
        let public_path = std::iter::once(package.to_owned())
            .chain(surface_module.split('.').skip(1).map(str::to_owned))
            .chain(std::iter::once(surface_types::as_str(surface).to_owned()))
            .collect::<Vec<_>>();
        let canonical = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_path(&public_path)?;
        let SymbolOrigin::Package { library, module_path } = &canonical.origin else {
            return None;
        };
        if !surface_types::is_exported_from_package(surface, library, module_path) {
            return None;
        }
        let mut native_path = surface_types::info_for(surface)
            .ownership
            .runtime_rust_module_path?
            .to_vec();
        native_path.push(surface_types::as_str(surface));
        incan_semantics_core::body_ir::rust_item_identity(&native_path.join("::"))
    }

    /// Preserve accepted root builtin bindings solely for direct-route storage; existing nominal and legacy facts stay
    /// unchanged.
    pub(super) fn checked_body_ir_runtime_types(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        self.symbols
            .active_module_symbols()
            .filter_map(|(id, symbol)| {
                let identity = self.symbols.identity_of(id)?;
                incan_semantics_core::RuntimeBuiltinType::from_identity(identity)?;
                Some((symbol.name.clone(), identity.clone()))
            })
            .collect()
    }

    /// Retain active module-scope nominal bindings from both source modules and admitted packages.
    ///
    /// Imported source types already carry their declaring identity in the symbol table. Body IR must retain that
    /// fact before publication; recovering it from a spelling in lowering would lose aliases and module ownership.
    pub(super) fn collect_checked_nominal_scope(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self.checked_nominal_bindings();
        // Immutable declaration scopes contribute through their admission index, not a scan of every prior snapshot.
        let carriers = identities
            .values()
            .filter_map(|identity| {
                self.retained_nominal_carriers
                    .get(identity)
                    .cloned()
                    .or_else(|| incan_semantics_core::body_ir::nominal_carrier_key(identity))
                    .map(|key| (key, identity.clone()))
            })
            .collect::<Vec<_>>();
        for (key, identity) in carriers.into_iter().chain(
            self.retained_nominal_carriers
                .iter()
                .map(|(identity, key)| (key.clone(), identity.clone())),
        ) {
            identities.entry(key).or_insert(identity);
        }
        identities
    }

    /// Collect current lexical and public nominal owners, preserving public precedence and ambiguous alias removal.
    /// The symbol owner already indexes active bindings; historical rejected and dependency rows are never scanned.
    pub(super) fn checked_nominal_bindings(&self) -> std::collections::BTreeMap<String, CanonicalSymbolId> {
        let mut identities = self
            .public_library_type_identities
            .iter()
            .filter_map(|(name, identity)| Some((name.clone(), identity.canonical.clone()?)))
            .collect::<std::collections::BTreeMap<_, _>>();
        for (id, symbol) in self.symbols.active_module_symbols() {
            let Some(identity) = self.symbols.identity_of(id) else {
                continue;
            };
            if matches!(
                identity.kind,
                SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::Newtype
            ) {
                identities
                    .entry(symbol.name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        let mut ambiguous = HashSet::new();
        for (name, alias) in &self.type_aliases {
            let active_alias = self.symbols.lookup(name).and_then(|id| self.symbols.get(id));
            if !active_alias.is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Type(TypeInfo::TypeAlias))) {
                continue;
            }
            for (leaf, identity) in &alias.nominal_identities {
                if identities.get(leaf).is_some_and(|bound| bound != identity) {
                    ambiguous.insert(leaf.clone());
                } else {
                    identities.insert(leaf.clone(), identity.clone());
                }
            }
        }
        for name in ambiguous {
            identities.remove(&name);
        }
        identities
    }

    /// Reuse the current scope only while all owning revisions match, including in-place alias target mutations.
    pub(super) fn checked_body_nominal_scope(
        &self,
    ) -> Option<Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>> {
        let scope = self.checking_nominal_scope.borrow();
        let scope = scope.as_ref()?;
        (scope.inputs == self.nominal_scope_inputs()).then(|| Arc::clone(&scope.identities))
    }

    /// Read constant-size mutation witnesses instead of checking each public owner and alias binding at every use.
    pub(super) fn nominal_scope_inputs(&self) -> NominalScopeInputs {
        NominalScopeInputs {
            symbols: self.symbols.nominal_revision(),
            aliases: self.type_aliases.revision(),
            public: self.public_library_type_identities.revision(),
            declarations: self.declaration_nominal_scopes.len(),
            carriers: self.retained_nominal_carriers.len(),
        }
    }

    /// Turn collisions from the shared symbol-registration mechanism into source diagnostics.
    ///
    /// The mechanism decides collision and ambiguity once; this layer owns user-facing wording. Two imports are
    /// ambiguous when their local spellings collide and their canonical identities differ or either target is
    /// unproven. A repeated binding to the same proven declaration is still a duplicate definition, not a new
    /// semantic declaration.
    pub(super) fn record_binding_collision_diagnostics(&mut self) {
        for collision in self.symbols.take_binding_collisions() {
            let immutable_builtin = collision.existing_identity.as_ref().is_some_and(|identity| {
                matches!(identity.origin, incan_semantics_core::SymbolOrigin::Builtin)
                    && identity.declaration_name == builtins::as_str(BuiltinFnId::Print)
            });
            let ambiguous_import = collision.existing_is_import
                && collision.incoming_is_import
                && (collision.existing_identity.is_none()
                    || collision.incoming_identity.is_none()
                    || collision.existing_identity != collision.incoming_identity);
            let mut error = if immutable_builtin {
                errors::immutable_builtin_redefinition(&collision.name, collision.incoming_span)
            } else if ambiguous_import {
                errors::ambiguous_import_binding(&collision.name, collision.existing_span, collision.incoming_span)
            } else {
                errors::duplicate_binding(&collision.name, collision.existing_span, collision.incoming_span)
            };
            if let Some(identity) = &collision.existing_identity {
                error = error.with_note(format!("First canonical identity: {}", identity.render_compact()));
            }
            if let Some(identity) = &collision.incoming_identity {
                error = error.with_note(format!("Second canonical identity: {}", identity.render_compact()));
            }
            self.errors.push(error);
        }
    }
}
