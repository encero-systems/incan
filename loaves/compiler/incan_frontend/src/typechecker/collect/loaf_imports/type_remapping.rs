//! Public library type remapping, type origins, artifact layouts, and nominal identities.

use super::deferred_layouts::DeclaringScopes;
use super::*;
use crate::typechecker::TransitiveLayout;
use crate::typechecker::transitive_layouts::{LayoutDeclaration, LayoutRecipe};

impl TypeChecker {
    /// Build the provider-aware type remapping shared by every import statement for one compiled library.
    ///
    /// Already-checked type imports can retain their local spelling in a later callable import. Provider-owned
    /// signature types without an active local binding receive an internal qualified spelling, and a later type import
    /// records the same provider identity for its local spelling. Both declaration orders therefore compare as the
    /// same nominal type without consulting a raw whole-program import scan.
    pub(in crate::typechecker::collect) fn public_library_type_import_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        let mut local_names_by_public_export: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let root = self.loaf_binding_root(library);
        for (local_name, path) in self.symbols.active_import_binding_paths() {
            let Some([public_name]) = path.strip_prefix(root.as_slice()) else {
                continue;
            };
            if !self.manifest_public_name_is_type(manifest, public_name) {
                continue;
            }
            local_names_by_public_export
                .entry(public_name.clone())
                .or_default()
                .push(local_name.to_string());
        }
        for local_names in local_names_by_public_export.values_mut() {
            local_names.sort();
            local_names.dedup();
        }

        let mut remapping = self.public_library_canonical_type_remapping(library, manifest);
        for (public_name, local_names) in local_names_by_public_export {
            if let Some(identity) = self.public_library_nominal_type_identity(library, manifest, &public_name) {
                for local_name in &local_names {
                    self.public_library_type_identities
                        .insert(local_name.clone(), identity.clone());
                }
            }
            if let Some(local_name) = local_names.into_iter().next() {
                remapping.insert(public_name, local_name);
            }
        }
        remapping
    }

    /// Build an import-scope-independent provider-qualified remapping for one compiled library.
    ///
    /// Qualified module access has no local `from` import whose alias can carry identity, so every nominal manifest
    /// spelling is mapped to its checker-owned canonical key before the member's signature enters expression checking.
    pub(in crate::typechecker) fn public_library_canonical_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        let prepared = self.prepared_canonical_type_remapping(library, manifest);
        let mut remapping = HashMap::with_capacity(prepared.len());
        for entry in prepared.iter() {
            self.public_library_type_identities
                .insert(entry.canonical_name.clone(), entry.identity.clone());
            remapping.insert(entry.public_name.clone(), entry.canonical_name.clone());
        }
        remapping.extend(self.public_library_foreign_type_remapping(library, manifest));
        remapping
    }

    /// Evaluate one compiled library's nominal public types once for the fresh checkers of one module graph.
    ///
    /// Whether a public export names a nominal type, and the provider identity it is admitted under, are facts of the
    /// library's manifest and the shared Loaf index, never of the asking checker's declarations or imports. Every
    /// consumer replays the same entries, in identity-graph order, into its own identity table.
    pub(in crate::typechecker::collect) fn prepared_canonical_type_remapping(
        &self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> Arc<[crate::typechecker::dependency_interface::CanonicalTypeEntry]> {
        let prepared = &self.dependency_interface_preparation_cache.canonical_remappings;
        if let Some(entries) = prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(library)
        {
            return Arc::clone(entries);
        }
        #[cfg(test)]
        self.dependency_interface_preparation_cache
            .count_canonical_evaluation(library);
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::prepare_canonical_type_remapping",
            library,
        )
        .entered();
        let mut entries = Vec::new();
        let mut seen = HashSet::new();
        for entry in &manifest.contract_metadata.identity_graph.exports {
            let public_name = entry.public_name.as_str();
            if entry.public_path.len() != 2 || !seen.insert(public_name) {
                continue;
            }
            let Some(identity) = self.public_library_nominal_type_identity(library, manifest, public_name) else {
                continue;
            };
            entries.push(crate::typechecker::dependency_interface::CanonicalTypeEntry {
                public_name: public_name.to_owned(),
                canonical_name: canonical_public_library_type_name(library, public_name),
                identity,
            });
        }
        let entries: Arc<[_]> = entries.into();
        Arc::clone(
            prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(library.to_owned())
                .or_insert(entries),
        )
    }

    /// Hydrate foreign signature types from the Loaves that declare them.
    ///
    /// Leaf binding keys carry identity; this map only chooses the consumer spelling, `pub::<loaf key>::<public
    /// path>`. Required nominal layouts come from the declaring Loaf's public identity graph, including types never
    /// imported by source name.
    ///
    /// The work is three phases and their order is the correctness argument. Seeding finds the origins this
    /// manifest can reach directly; the closure walks from those to every Loaf transitively required; only then
    /// can layouts be reconstructed, because a layout may reference a type discovered late, and remapping it
    /// against a partial map would bake in a name that never resolves.
    pub(in crate::typechecker::collect) fn public_library_foreign_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> HashMap<String, String> {
        if let Some(remapping) = self.foreign_pub_type_remappings.get(library) {
            return remapping.clone();
        }
        let prepared = self.prepared_foreign_remapping(library, manifest);
        self.install_prepared_foreign_remapping(&prepared);
        let remapping = prepared.remapping.as_ref().clone();
        self.foreign_pub_type_remappings
            .insert(library.to_string(), remapping.clone());
        remapping
    }

    /// Build one compiled Loaf's foreign nominal projection once, then share its immutable effects across the fresh
    /// checkers of this module graph.
    fn prepared_foreign_remapping(
        &self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> Arc<crate::typechecker::dependency_interface::PreparedForeignRemapping> {
        let cached = match self.dependency_interface_preparation_cache.foreign_remappings.lock() {
            Ok(cache) => cache.get(library).cloned(),
            Err(poisoned) => poisoned.into_inner().get(library).cloned(),
        };
        if let Some(cached) = cached {
            return cached;
        }

        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::prepare_compiled_dependency_interface",
            library,
        )
        .entered();
        let mut builder = TypeChecker::new();
        builder.set_loaves(Arc::clone(&self.loaves));
        // The builder prepares for this build, so it shares the build's preparations of the Loaves it reaches.
        builder.set_dependency_interface_preparation_cache(self.dependency_interface_preparation_cache.clone());
        let (remapping, preserving_identity_keys, preserving_type_keys) =
            builder.build_foreign_remapping(library, manifest);
        let mut replacing_identities = HashMap::new();
        let mut preserving_identities = HashMap::new();
        for (name, identity) in &builder.public_library_type_identities {
            if preserving_identity_keys.contains(name) {
                preserving_identities.insert(name.clone(), identity.clone());
            } else {
                replacing_identities.insert(name.clone(), identity.clone());
            }
        }
        let mut appending_types = HashMap::new();
        let mut preserving_types = HashMap::new();
        for (name, types) in builder.transitive_pub_types {
            if preserving_type_keys.contains(&name) {
                preserving_types.insert(name, types);
            } else {
                appending_types.insert(name, types);
            }
        }
        let prepared = Arc::new(crate::typechecker::dependency_interface::PreparedForeignRemapping {
            remapping: Arc::new(remapping),
            replacing_identities: Arc::new(replacing_identities),
            preserving_identities: Arc::new(preserving_identities),
            appending_types: Arc::new(appending_types),
            preserving_types: Arc::new(preserving_types),
            errors: Arc::from(builder.errors),
        });
        match self.dependency_interface_preparation_cache.foreign_remappings.lock() {
            Ok(mut cache) => Arc::clone(
                cache
                    .entry(library.to_string())
                    .or_insert_with(|| Arc::clone(&prepared)),
            ),
            Err(poisoned) => {
                let mut cache = poisoned.into_inner();
                Arc::clone(
                    cache
                        .entry(library.to_string())
                        .or_insert_with(|| Arc::clone(&prepared)),
                )
            }
        }
    }

    /// Replay one prepared compiled dependency under the receiving checker's existing collision rules.
    fn install_prepared_foreign_remapping(
        &mut self,
        prepared: &crate::typechecker::dependency_interface::PreparedForeignRemapping,
    ) {
        self.public_library_type_identities.extend(
            prepared
                .replacing_identities
                .iter()
                .map(|(name, identity)| (name.clone(), identity.clone())),
        );
        for (name, identity) in prepared.preserving_identities.iter() {
            self.public_library_type_identities
                .entry(name.clone())
                .or_insert_with(|| identity.clone());
        }
        for (name, values) in prepared.appending_types.iter() {
            self.transitive_pub_types
                .entry(name.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
        for (name, values) in prepared.preserving_types.iter() {
            self.transitive_pub_types
                .entry(name.clone())
                .or_insert_with(|| values.clone());
        }
        self.errors.extend(prepared.errors.iter().cloned());
    }

    /// Compute the immutable foreign nominal projection in an otherwise empty checker.
    fn build_foreign_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> (HashMap<String, String>, HashSet<String>, HashSet<String>) {
        let seeds = self.loaf_type_origins(library, manifest);
        let (remapping, loaves) = self.resolve_public_type_origins(library, seeds);
        let (preserving_identity_keys, preserving_type_keys) =
            self.reconstruct_artifact_layouts(library, &remapping, &loaves);
        (remapping, preserving_identity_keys, preserving_type_keys)
    }

    /// Walk from the seed origins to every Loaf transitively required, registering each type's spelling.
    ///
    /// Resolving one origin can reveal a Loaf whose own manifest names further origins, so this is a worklist rather
    /// than a pass. `seen` is keyed by binding key so a type reached by two routes is resolved once, which is what
    /// keeps mutual references from looping. An origin no indexed Loaf declares is reported and skipped rather than
    /// failing the whole map, so a single bad type does not cost every other import in the library.
    pub(in crate::typechecker::collect) fn resolve_public_type_origins(
        &mut self,
        library: &str,
        seeds: Arc<BTreeMap<String, NominalTypeOriginExport>>,
    ) -> (HashMap<String, String>, BTreeMap<String, LoafEntry>) {
        let mut pending = std::collections::VecDeque::from_iter(seeds.values().cloned());
        let mut seen = HashSet::new();
        let mut loaves = BTreeMap::new();
        let mut remapping = HashMap::new();

        while let Some(origin) = pending.pop_front() {
            let key = origin.binding_key();
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(canonical) = origin.canonical.hydrate() else {
                continue;
            };
            let Some((entry, export)) = self
                .loaves
                .public_nominal(&canonical)
                .map(|(entry, export)| (entry.clone(), export.clone()))
            else {
                self.push_public_library_error(
                    library,
                    format!("names a public type no indexed Loaf declares: {canonical:?}"),
                );
                continue;
            };
            let qualified = canonical_public_library_type_name(&entry.key, &export.public_path[1..].join("::"));
            let identity = PublicLibraryTypeIdentity {
                dependency_key: library.to_string(),
                source_path: export.source_path.clone(),
                canonical: Some(canonical),
                selected_provider: Some(entry.artifact.identity.clone()),
            };
            self.public_library_type_identities
                .insert(key.clone(), identity.clone());
            self.public_library_type_identities.insert(qualified.clone(), identity);
            remapping.insert(key, qualified);

            if let std::collections::btree_map::Entry::Vacant(vacant) = loaves.entry(entry.key.clone()) {
                pending.extend(
                    self.loaf_type_origins(&entry.key, &entry.artifact.manifest)
                        .values()
                        .cloned(),
                );
                vacant.insert(entry);
            }
        }
        (remapping, loaves)
    }

    /// The nominal type origins the manifest of the Loaf indexed as `key` references, keyed by binding key: a fact of
    /// the Loaf, collected once for the checkers of one module graph however many foreign preparations reach it.
    pub(in crate::typechecker::collect) fn loaf_type_origins(
        &self,
        key: &str,
        manifest: &LibraryManifest,
    ) -> Arc<BTreeMap<String, NominalTypeOriginExport>> {
        let collected = &self.dependency_interface_preparation_cache.type_origins;
        if let Some(origins) = collected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
        {
            return Arc::clone(origins);
        }
        #[cfg(test)]
        self.dependency_interface_preparation_cache.count_origin_collection(key);
        let _trace = tracing::trace_span!(
            target: "incan::invocation",
            "incan_frontend::collect_loaf_type_origins",
            loaf = key,
        )
        .entered();
        let origins = Arc::new(collect_nominal_type_origins(manifest));
        Arc::clone(
            collected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(key.to_owned())
                .or_insert(origins),
        )
    }

    /// Rebuild each discovered Loaf's nominal layouts against the completed remapping.
    ///
    /// This runs only after every type mapping exists, which is what preserves mutually referenced layouts and
    /// foreign field, property and default signatures without reopening Loaves or looking up source. The same
    /// ordering holds inside each Loaf: every export's public name is registered first, so the second pass can
    /// remap a layout that refers to a sibling declared after it.
    pub(in crate::typechecker::collect) fn reconstruct_artifact_layouts(
        &mut self,
        library: &str,
        remapping: &HashMap<String, String>,
        loaves: &BTreeMap<String, LoafEntry>,
    ) -> (HashSet<String>, HashSet<String>) {
        let mut preserving_identity_keys = HashSet::new();
        let mut preserving_type_keys = HashSet::new();
        for entry in loaves.values() {
            let (local_remapping, entry_preserving_keys) = self.register_artifact_type_names(library, entry, remapping);
            preserving_identity_keys.extend(entry_preserving_keys);
            preserving_type_keys.extend(self.rebuild_artifact_symbol_layouts(entry, local_remapping));
        }
        (preserving_identity_keys, preserving_type_keys)
    }

    /// Register every nominal export of one artifact under its qualified consumer name.
    ///
    /// Returns the remapping extended with this artifact's own top-level spellings, so a layout rebuilt next can
    /// resolve a sibling type by the bare name its signature uses. A top-level spelling is a declaration of the
    /// package root or a root re-export of a submodule's declaration (`pub from search import Nomination`): both are
    /// the name a published signature uses for the type, and a re-export left out would leave a field typed by it
    /// with no members.
    pub(in crate::typechecker::collect) fn register_artifact_type_names(
        &mut self,
        library: &str,
        entry: &LoafEntry,
        remapping: &HashMap<String, String>,
    ) -> (HashMap<String, String>, HashSet<String>) {
        let mut local_remapping = remapping.clone();
        let mut preserving_keys = HashSet::new();
        let artifact = &entry.artifact;
        let owner_route = entry.key.as_str();
        let graph = &artifact.manifest.contract_metadata.identity_graph;
        for export in &graph.exports {
            let declared = matches!(
                export.kind,
                crate::library_manifest::ExportIdentityKind::Model
                    | crate::library_manifest::ExportIdentityKind::Class
                    | crate::library_manifest::ExportIdentityKind::Enum
                    | crate::library_manifest::ExportIdentityKind::Newtype
            );
            let top_level = export.public_path.len() == 2;
            let reexported = !declared
                && top_level
                && self
                    .manifest_nominal_type_info(&artifact.manifest, &export.public_name)
                    .is_some();
            if !declared && !reexported {
                continue;
            }
            let qualified = canonical_public_library_type_name(owner_route, &export_public_name(export));
            let canonical = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            // A re-export names the declaration its canonical identity points at, so its identity carries that
            // declaration's own source path rather than the re-export hop.
            let source_path = if reexported {
                canonical
                    .as_ref()
                    .and_then(|canonical| graph.declaration_source_path(canonical))
                    .map_or_else(|| export.source_path.clone(), <[String]>::to_vec)
            } else {
                export.source_path.clone()
            };
            let identity = PublicLibraryTypeIdentity {
                dependency_key: library.to_string(),
                source_path,
                canonical,
                selected_provider: Some(artifact.identity.clone()),
            };
            if reexported {
                if let std::collections::hash_map::Entry::Vacant(vacant) =
                    self.public_library_type_identities.entry(qualified.clone())
                {
                    vacant.insert(identity);
                    preserving_keys.insert(qualified.clone());
                }
            } else {
                self.public_library_type_identities.insert(qualified.clone(), identity);
            }
            // Only a top-level export is reachable by a bare name in a signature; a nested one is always written
            // through its path, so adding it here would shadow the wrong spelling.
            if top_level {
                local_remapping.insert(export.public_name.clone(), qualified);
            }
        }
        (local_remapping, preserving_keys)
    }

    /// Rebuild one artifact's exported symbol layouts with foreign type names already resolved.
    ///
    /// Member types are read in the declaring module's scope before the artifact's top-level spellings, so a layout
    /// names the declaration its own module names whether the root republishes it or not. Retained carriers receive the
    /// same published layout even when no source import binds the type.
    pub(in crate::typechecker::collect) fn rebuild_artifact_symbol_layouts(
        &mut self,
        entry: &LoafEntry,
        local_remapping: HashMap<String, String>,
    ) -> HashSet<String> {
        let artifact = &entry.artifact;
        let manifest = &artifact.manifest;
        let owner_route = entry.key.as_str();
        let local_remapping = Arc::new(local_remapping);
        let mut scopes = DeclaringScopes::new();
        let mut preserving_type_keys = HashSet::new();
        for export in &manifest.contract_metadata.identity_graph.exports {
            // A member type is spelled as the declaring module spells it; the names that module reaches through a
            // private import or a submodule the root does not republish resolve through its scope first.
            let declaration = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            let Some(layout) =
                self.artifact_export_layout(entry, export, declaration.as_ref(), &local_remapping, &mut scopes)
            else {
                continue;
            };
            if let Some(carrier) = declaration
                .as_ref()
                .and_then(incan_semantics_core::body_ir::nominal_carrier_key)
            {
                preserving_type_keys.insert(carrier);
            }
            let qualified = canonical_public_library_type_name(owner_route, &export_public_name(export));
            self.cache_retained_nominal_layout(declaration.as_ref(), &layout);
            self.transitive_pub_types.entry(qualified).or_default().push(layout);
        }
        preserving_type_keys
    }

    /// One exported type's layout in `entry`, its members read in its declaring module's scope and then through
    /// `local_remapping`. `None` for an export that names no type.
    ///
    /// A top-level export is described by the manifest's own nominal info; a nested one has to be read back from its
    /// declaration, because only the declaration records the members. A model, class, enum or newtype is deferred to
    /// the first lookup that reads it; an alias reads the tables, so it is built now, as is a type alias.
    fn artifact_export_layout(
        &mut self,
        entry: &LoafEntry,
        export: &crate::library_manifest::ExportIdentity,
        declaration: Option<&CanonicalSymbolId>,
        local_remapping: &Arc<HashMap<String, String>>,
        scopes: &mut DeclaringScopes,
    ) -> Option<TransitiveLayout<TypeInfo>> {
        let manifest = &entry.artifact.manifest;
        let owner_route = entry.key.as_str();
        let (deferred, built) = if export.public_path.len() == 2 {
            if Self::export_layout_is_deferrable(manifest, &export.public_name) {
                (Some(LayoutDeclaration::Export(export.public_name.clone())), None)
            } else {
                (
                    None,
                    Some(self.manifest_nominal_type_info(manifest, &export.public_name)?),
                )
            }
        } else {
            let api_declaration = Self::api_declaration_for_target_path(manifest, &export.source_path)?;
            match api_declaration {
                ApiDeclaration::Model(_)
                | ApiDeclaration::Class(_)
                | ApiDeclaration::Enum(_)
                | ApiDeclaration::Newtype(_) => (Some(LayoutDeclaration::ApiPath(export.source_path.clone())), None),
                _ => {
                    let api = manifest.contract_metadata.api.as_ref();
                    let SymbolKind::Type(info) = self.symbol_kind_from_api_declaration(api, api_declaration)? else {
                        return None;
                    };
                    (None, Some(info))
                }
            }
        };
        let scope = self.declaring_module_scope(owner_route, manifest, declaration, scopes);
        if let Some(position) = deferred {
            return Some(self.deferred_layout(LayoutRecipe {
                manifest: Arc::clone(manifest),
                declaration: position,
                remappings: scope.into_iter().chain([Arc::clone(local_remapping)]).collect(),
                field_provider: Some(owner_route.to_owned()),
            }));
        }
        let mut kind = SymbolKind::Type(built?);
        if let Some(scope) = scope {
            self.remap_symbol_kind_with_import_aliases(&mut kind, scope.as_ref());
        }
        self.remap_symbol_kind_with_import_aliases(&mut kind, local_remapping.as_ref());
        Self::mark_compiled_class_field_provider(&mut kind, owner_route);
        let SymbolKind::Type(info) = kind else {
            return None;
        };
        Some(TransitiveLayout::ready(info))
    }

    /// Report a malformed compiled-library input without repeating the diagnostic's shape at each call.
    pub(in crate::typechecker::collect) fn push_public_library_error(&mut self, library: &str, detail: String) {
        self.errors.push(crate::diagnostics::CompileError::type_error(
            format!("compiled library `{library}` {detail}"),
            Span::default(),
        ));
    }

    /// Return the nominal origin carried by a type reference, when it has one.
    pub(in crate::typechecker::collect) fn nominal_type_origin(
        ty: &crate::library_manifest::TypeRef,
    ) -> Option<&NominalTypeOriginExport> {
        use crate::library_manifest::TypeRef;
        match ty {
            TypeRef::Named {
                origin: Some(origin), ..
            }
            | TypeRef::Applied {
                origin: Some(origin), ..
            } => Some(origin),
            _ => None,
        }
    }

    /// Return whether one public manifest spelling resolves to a type-like import binding.
    pub(in crate::typechecker::collect) fn manifest_public_name_is_type(
        &self,
        manifest: &LibraryManifest,
        public_name: &str,
    ) -> bool {
        let Some(export) = Self::find_manifest_export(manifest, public_name) else {
            return false;
        };
        match export {
            ManifestExportRef::Alias(alias) => self
                .symbol_kind_from_manifest_alias(manifest, alias, &mut HashSet::new())
                .is_some_and(|kind| matches!(kind, SymbolKind::Type(_) | SymbolKind::Trait(_))),
            other => Self::manifest_export_is_type(&other),
        }
    }

    /// Resolve one public spelling to nominal model/class/enum/newtype metadata.
    pub(in crate::typechecker) fn manifest_nominal_type_info(
        &self,
        manifest: &LibraryManifest,
        public_name: &str,
    ) -> Option<TypeInfo> {
        let export = Self::find_manifest_export(manifest, public_name)?;
        let kind = match export {
            ManifestExportRef::Alias(alias) => {
                if Self::alias_names_no_nominal_type(manifest, alias) {
                    return None;
                }
                self.symbol_kind_from_manifest_alias(manifest, alias, &mut HashSet::new())?
            }
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::Newtype(export) => {
                SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
            }
            _ => return None,
        };
        match kind {
            SymbolKind::Type(
                info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_)),
            ) => Some(info),
            _ => None,
        }
    }

    /// Resolve one public nominal type spelling to its dependency-qualified provider source identity.
    pub(in crate::typechecker::collect) fn public_library_nominal_type_identity(
        &self,
        library: &str,
        manifest: &LibraryManifest,
        public_name: &str,
    ) -> Option<PublicLibraryTypeIdentity> {
        let entry = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)?;
        match Self::find_manifest_export(manifest, public_name)? {
            ManifestExportRef::Model(_)
            | ManifestExportRef::Class(_)
            | ManifestExportRef::Enum(_)
            | ManifestExportRef::Newtype(_) => {}
            ManifestExportRef::Alias(_) => {
                self.manifest_nominal_type_info(manifest, public_name)?;
            }
            _ => return None,
        }
        let source_path = entry.target_path().unwrap_or(entry.source_path.as_slice());
        Some(self.admitted_public_type_identity(
            library,
            source_path,
            entry.canonical.as_ref().and_then(|identity| identity.hydrate()),
        ))
    }

    /// Return the missing public features for one known provider export that is inactive in this artifact projection.
    pub(in crate::typechecker::collect) fn inactive_pub_export_features(
        manifest: &LibraryManifest,
        symbol: &str,
    ) -> Option<Vec<Vec<String>>> {
        let active = &manifest.contract_metadata.provider.active_features;
        let mut alternatives = manifest
            .contract_metadata
            .provider
            .fact_requirements
            .iter()
            .filter(|requirement| {
                requirement.kind == ProviderFactKind::Export
                    && requirement.identity.rsplit("::").next() == Some(symbol)
                    && !requirement.required_features.is_subset(active)
            })
            .map(|requirement| {
                requirement
                    .required_features
                    .difference(active)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        alternatives.sort();
        alternatives.dedup();
        (!alternatives.is_empty()).then_some(alternatives)
    }
}
