//! Public library type remapping, type origins, artifact layouts, and nominal identities.

use super::deferred_layouts::DeclaringScopes;
use super::*;
use crate::library_manifest::install_record::{
    InstallExportKind, InstallNestedLayoutKind, LoafInstallRecord, is_nominal_export,
};
use crate::typechecker::TransitiveLayout;
use crate::typechecker::transitive_layouts::{LayoutDeclaration, LayoutRecipe};

/// One identity export of a reached Loaf, with the kind of root export its name finds and the Loaf's identity graph,
/// as the Loaf's install record gives them.
#[derive(Clone, Copy)]
struct ArtifactExport<'a> {
    library: &'a str,
    record: &'a LoafInstallRecord,
    export: &'a crate::library_manifest::ExportIdentity,
    kind: Option<InstallExportKind>,
    nested: Option<&'a InstallNestedLayoutKind>,
}

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

        let mut remapping = self.public_library_canonical_type_remapping(library);
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
    ) -> HashMap<String, String> {
        let Some(record) = self.prepared_install_record(library) else {
            return HashMap::new();
        };
        self.replay_library_type_remapping(library, &record)
    }

    /// Install the canonical names and foreign nominal layouts carried by this Loaf's record.
    pub(super) fn replay_library_type_remapping(
        &mut self,
        library: &str,
        record: &Arc<LoafInstallRecord>,
    ) -> HashMap<String, String> {
        let prepared = self.prepared_canonical_type_remapping(library, record);
        let mut remapping = HashMap::with_capacity(prepared.len());
        for entry in prepared.iter() {
            self.public_library_type_identities
                .insert(entry.canonical_name.clone(), entry.identity.clone());
            remapping.insert(entry.public_name.clone(), entry.canonical_name.clone());
        }
        remapping.extend(self.public_library_foreign_type_remapping(library, record));
        remapping
    }

    /// One compiled library's nominal public types, read once from its install record for the fresh checkers of one
    /// module graph.
    ///
    /// Whether a public export names a nominal type, and the declaration it names, are facts of the Loaf's install
    /// record; the provider identity each is admitted under is the shared Loaf index's, never the asking checker's
    /// declarations or imports. Every consumer replays the same entries, in identity-graph order, into its own identity
    /// table.
    pub(in crate::typechecker::collect) fn prepared_canonical_type_remapping(
        &self,
        library: &str,
        record: &LoafInstallRecord,
    ) -> Arc<[crate::typechecker::dependency_interface::CanonicalTypeEntry]> {
        let prepared = &self.dependency_interface_preparation_cache.canonical_remappings;
        if let Some(entries) = prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(library)
        {
            return Arc::clone(entries);
        }
        let entries: Arc<[_]> = record
            .canonical_types
            .iter()
            .map(|row| crate::typechecker::dependency_interface::CanonicalTypeEntry {
                public_name: row.public_name.clone(),
                canonical_name: canonical_public_library_type_name(library, &row.public_name),
                identity: self.replayed_type_identity(
                    library,
                    &row.source_path,
                    row.canonical
                        .as_ref()
                        .and_then(crate::library_manifest::CanonicalIdentityExport::hydrate),
                ),
            })
            .collect::<Vec<_>>()
            .into();
        Arc::clone(
            prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(library.to_owned())
                .or_insert(entries),
        )
    }

    /// Bind a recorded nominal row under its consumer key and the selected provider's admitted identity.
    pub(in crate::typechecker::collect) fn replayed_type_identity(
        &self,
        library: &str,
        source_path: &[String],
        canonical: Option<CanonicalSymbolId>,
    ) -> PublicLibraryTypeIdentity {
        let provider = match canonical.as_ref() {
            Some(canonical) => self.install_declaring_loaf(canonical).map(|(key, _)| key),
            None => Some(library),
        };
        let mut identity = PublicLibraryTypeIdentity::new(library, source_path).with_canonical(canonical);
        identity.selected_provider = provider
            .and_then(|key| self.loaves.package_identity(key).ok().flatten())
            .cloned();
        identity
    }

    /// The nominal public types of the compiled library `library` as its manifest publishes them: each unique
    /// top-level public name that names a nominal type, in identity-graph order, with the identity it is admitted
    /// under. Its install record lists these.
    pub(in crate::typechecker::collect) fn manifest_nominal_public_types(
        &self,
        library: &str,
        manifest: &LibraryManifest,
    ) -> Vec<crate::typechecker::dependency_interface::CanonicalTypeEntry> {
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
        entries
    }

    /// Hydrate foreign signature types from the Loaves that declare them.
    ///
    /// Leaf binding keys carry identity; this map only chooses the consumer spelling, `pub::<loaf key>::<public
    /// path>`. Required nominal layouts come from the declaring Loaf's public identity graph, including types never
    /// imported by source name.
    ///
    /// The work is three phases and their order is the correctness argument. Seeding finds the origins this
    /// record can reach directly; the closure walks from those to every Loaf transitively required; only then
    /// can layouts be reconstructed, because a layout may reference a type discovered late, and remapping it
    /// against a partial map would bake in a name that never resolves.
    pub(in crate::typechecker::collect) fn public_library_foreign_type_remapping(
        &mut self,
        library: &str,
        record: &Arc<LoafInstallRecord>,
    ) -> HashMap<String, String> {
        if let Some(remapping) = self.foreign_pub_type_remappings.get(library) {
            return remapping.clone();
        }
        let prepared = self.prepared_foreign_remapping(library, record);
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
        record: &Arc<LoafInstallRecord>,
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
            builder.build_foreign_remapping(library, record);
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
        record: &Arc<LoafInstallRecord>,
    ) -> (HashMap<String, String>, HashSet<String>, HashSet<String>) {
        let (remapping, loaves) = self.resolve_public_type_origins(library, record);
        let (preserving_identity_keys, preserving_type_keys) =
            self.reconstruct_artifact_layouts(library, &remapping, &loaves);
        (remapping, preserving_identity_keys, preserving_type_keys)
    }

    /// Walk from the seed origins to every Loaf transitively required, registering each type's spelling.
    ///
    /// Resolving one origin can reveal a Loaf whose own record names further origins, so this is a worklist rather
    /// than a pass. `seen` is keyed by binding key so a type reached by two routes is resolved once, which is what
    /// keeps mutual references from looping. An origin no indexed Loaf declares is reported and skipped rather than
    /// failing the whole map, so a single bad type does not cost every other import in the library.
    pub(in crate::typechecker::collect) fn resolve_public_type_origins(
        &mut self,
        library: &str,
        record: &Arc<LoafInstallRecord>,
    ) -> (HashMap<String, String>, BTreeMap<String, Arc<LoafInstallRecord>>) {
        let mut pending = std::collections::VecDeque::from([Arc::clone(record)]);
        let mut seen = HashSet::new();
        let mut loaves = BTreeMap::new();
        let mut remapping = HashMap::new();

        while let Some(reached) = pending.pop_front() {
            for origin in &reached.type_origins {
                let key = origin.binding_key();
                if !seen.insert(key.clone()) {
                    continue;
                }
                let Some(canonical) = origin.canonical.hydrate() else {
                    continue;
                };
                let Some((provider, record, qualified, identity)) = self.replay_foreign_origin(library, &canonical)
                else {
                    self.push_public_library_error(
                        library,
                        format!("names a public type no indexed Loaf declares: {canonical:?}"),
                    );
                    continue;
                };
                self.public_library_type_identities
                    .insert(key.clone(), identity.clone());
                self.public_library_type_identities.insert(qualified.clone(), identity);
                remapping.insert(key, qualified);

                if let std::collections::btree_map::Entry::Vacant(vacant) = loaves.entry(provider) {
                    pending.push_back(Arc::clone(&record));
                    vacant.insert(record);
                }
            }
        }
        (remapping, loaves)
    }

    /// Resolve a foreign nominal through its declaring Loaf's record and exact admitted provider identity.
    fn replay_foreign_origin(
        &self,
        library: &str,
        canonical: &CanonicalSymbolId,
    ) -> Option<(String, Arc<LoafInstallRecord>, String, PublicLibraryTypeIdentity)> {
        let (provider, record) = self.install_declaring_loaf(canonical)?;
        let export = record.identity_graph.public_nominal(canonical)?;
        let provider_identity = self.loaves.package_identity(provider).ok()??;
        let qualified = canonical_public_library_type_name(provider, &export_public_name(export));
        let identity = PublicLibraryTypeIdentity {
            dependency_key: library.to_owned(),
            source_path: export.source_path.clone(),
            canonical: Some(canonical.clone()),
            selected_provider: Some(provider_identity.clone()),
        };
        Some((provider.to_owned(), record, qualified, identity))
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
        loaves: &BTreeMap<String, Arc<LoafInstallRecord>>,
    ) -> (HashSet<String>, HashSet<String>) {
        let mut preserving_identity_keys = HashSet::new();
        let mut preserving_type_keys = HashSet::new();
        for (key, record) in loaves {
            let (local_remapping, entry_preserving_keys) =
                self.register_artifact_type_names(library, key, record, remapping);
            preserving_identity_keys.extend(entry_preserving_keys);
            preserving_type_keys.extend(self.rebuild_artifact_symbol_layouts(key, record, local_remapping));
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
        owner_route: &str,
        record: &LoafInstallRecord,
        remapping: &HashMap<String, String>,
    ) -> (HashMap<String, String>, HashSet<String>) {
        let mut local_remapping = remapping.clone();
        let mut preserving_keys = HashSet::new();
        let selected_provider = self.loaves.package_identity(owner_route).ok().flatten().cloned();
        let canonical_names: HashSet<&str> = record
            .canonical_types
            .iter()
            .map(|entry| entry.public_name.as_str())
            .collect();
        let kinds = record.export_kinds_by_name();
        let graph = &record.identity_graph;
        for export in &graph.exports {
            let declared = matches!(
                export.kind,
                crate::library_manifest::ExportIdentityKind::Model
                    | crate::library_manifest::ExportIdentityKind::Class
                    | crate::library_manifest::ExportIdentityKind::Enum
                    | crate::library_manifest::ExportIdentityKind::Newtype
            );
            let top_level = export.public_path.len() == 2;
            let kind = kinds.get(export.public_name.as_str()).copied().flatten();
            let reexported = !declared
                && top_level
                && (is_nominal_export(kind)
                    || kind == Some(InstallExportKind::Alias) && canonical_names.contains(export.public_name.as_str()));
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
                selected_provider: selected_provider.clone(),
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
        owner_route: &str,
        record: &LoafInstallRecord,
        local_remapping: HashMap<String, String>,
    ) -> HashSet<String> {
        let local_remapping = Arc::new(local_remapping);
        let kinds = record.export_kinds_by_name();
        let nested: HashMap<_, _> = record
            .nested_layouts
            .iter()
            .map(|layout| (layout.source_path.as_slice(), &layout.kind))
            .collect();
        let mut scopes = DeclaringScopes::new();
        let mut preserving_type_keys = HashSet::new();
        for export in &record.identity_graph.exports {
            // A member type is spelled as the declaring module spells it; the names that module reaches through a
            // private import or a submodule the root does not republish resolve through its scope first.
            let declaration = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            let kind = kinds.get(export.public_name.as_str()).copied().flatten();
            let source = ArtifactExport {
                library: owner_route,
                record,
                export,
                kind,
                nested: nested.get(export.source_path.as_slice()).copied(),
            };
            let Some(layout) =
                self.artifact_export_layout(&source, declaration.as_ref(), &local_remapping, &mut scopes)
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

    /// One exported type's layout in the Loaf's record, its members read in its declaring module's scope and then
    /// through `local_remapping`. `None` for an export that names no type.
    ///
    /// The record chooses the layout input. A model, class, enum or newtype is deferred to the first lookup that
    /// reads it. Alias projections can consult the current type tables, so they are resolved here.
    fn artifact_export_layout(
        &mut self,
        source: &ArtifactExport<'_>,
        declaration: Option<&CanonicalSymbolId>,
        local_remapping: &Arc<HashMap<String, String>>,
        scopes: &mut DeclaringScopes,
    ) -> Option<TransitiveLayout<TypeInfo>> {
        let ArtifactExport {
            library,
            record,
            export,
            kind,
            nested,
        } = *source;
        let (deferred, built) = if export.public_path.len() == 2 {
            if is_nominal_export(kind) {
                (Some(LayoutDeclaration::Export(export.public_name.clone())), None)
            } else if kind != Some(InstallExportKind::Alias) {
                return None;
            } else {
                (
                    None,
                    Some(self.recorded_root_alias_layout(library, record, &export.public_name)?),
                )
            }
        } else {
            match nested? {
                InstallNestedLayoutKind::Nominal => {
                    (Some(LayoutDeclaration::ApiPath(export.source_path.clone())), None)
                }
                InstallNestedLayoutKind::TypeAlias => (None, Some(TypeInfo::TypeAlias)),
                InstallNestedLayoutKind::Forwarded(projected) => {
                    let SymbolKind::Type(info) = self.symbol_kind_from_type_projection(Some(projected))? else {
                        return None;
                    };
                    (None, Some(info))
                }
            }
        };
        let scope = self.declaring_module_scope(library, record, declaration, scopes);
        if let Some(position) = deferred {
            return Some(self.deferred_layout(LayoutRecipe {
                library: library.to_owned(),
                declaration: position,
                remappings: scope.into_iter().chain([Arc::clone(local_remapping)]).collect(),
                field_provider: Some(library.to_owned()),
            }));
        }
        let mut kind = SymbolKind::Type(built?);
        if let Some(scope) = scope {
            self.remap_symbol_kind_with_import_aliases(&mut kind, scope.as_ref());
        }
        self.remap_symbol_kind_with_import_aliases(&mut kind, local_remapping.as_ref());
        Self::mark_compiled_class_field_provider(&mut kind, library);
        let SymbolKind::Type(info) = kind else {
            return None;
        };
        Some(TransitiveLayout::ready(info))
    }

    /// Resolve the recorded root alias to a nominal layout. Target layout reads still use the older manifest
    /// representation, while the choice of alias comes only from the install record.
    pub(super) fn recorded_root_alias_layout(
        &self,
        library: &str,
        record: &LoafInstallRecord,
        public_name: &str,
    ) -> Option<TypeInfo> {
        if !record
            .canonical_types
            .iter()
            .any(|entry| entry.public_name == public_name)
        {
            return None;
        }
        let alias = record.aliases.iter().find(|alias| alias.name == public_name)?;
        let manifest = self.loaves.manifest(library)?;
        nominal_layout(self.symbol_kind_from_manifest_alias(manifest, alias, &mut HashSet::new())?)
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
        nominal_layout(kind)
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

/// Return the transitive layout of the four nominal kinds admitted by compiled Loaf install.
fn nominal_layout(kind: SymbolKind) -> Option<TypeInfo> {
    match kind {
        SymbolKind::Type(
            info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_)),
        ) => Some(info),
        _ => None,
    }
}
