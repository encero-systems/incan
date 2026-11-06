//! Public library type remapping, type origins, artifact layouts, and nominal identities.

use super::*;

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
        let mut remapping = HashMap::new();
        let public_names = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .map(|entry| entry.public_name.clone())
            .collect::<Vec<_>>();
        for public_name in public_names {
            let Some(identity) = self.public_library_nominal_type_identity(library, manifest, &public_name) else {
                continue;
            };
            let canonical_name = canonical_public_library_type_name(library, &public_name);
            self.public_library_type_identities
                .insert(canonical_name.clone(), identity);
            remapping.insert(public_name, canonical_name);
        }
        remapping.extend(self.public_library_foreign_type_remapping(library, manifest));
        remapping
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
        let seeds = collect_nominal_type_origins(manifest);
        let (remapping, loaves) = self.resolve_public_type_origins(library, seeds);
        self.reconstruct_artifact_layouts(library, &remapping, &loaves);
        self.foreign_pub_type_remappings
            .insert(library.to_string(), remapping.clone());
        remapping
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
        seeds: BTreeMap<String, NominalTypeOriginExport>,
    ) -> (HashMap<String, String>, BTreeMap<String, LoafEntry>) {
        let mut pending = std::collections::VecDeque::from_iter(seeds.into_values());
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
                pending.extend(collect_nominal_type_origins(&entry.artifact.manifest).into_values());
                vacant.insert(entry);
            }
        }
        (remapping, loaves)
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
    ) {
        for entry in loaves.values() {
            let local_remapping = self.register_artifact_type_names(library, entry, remapping);
            self.rebuild_artifact_symbol_layouts(entry, &local_remapping);
        }
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
    ) -> HashMap<String, String> {
        let mut local_remapping = remapping.clone();
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
                self.public_library_type_identities
                    .entry(qualified.clone())
                    .or_insert(identity);
            } else {
                self.public_library_type_identities.insert(qualified.clone(), identity);
            }
            // Only a top-level export is reachable by a bare name in a signature; a nested one is always written
            // through its path, so adding it here would shadow the wrong spelling.
            if top_level {
                local_remapping.insert(export.public_name.clone(), qualified);
            }
        }
        local_remapping
    }

    /// Rebuild one artifact's exported symbol layouts with foreign type names already resolved.
    ///
    /// Member types are read in the declaring module's scope before the artifact's top-level spellings, so a layout
    /// names the declaration its own module names whether the root republishes it or not. Retained carriers receive the
    /// same published layout even when no source import binds the type.
    pub(in crate::typechecker::collect) fn rebuild_artifact_symbol_layouts(
        &mut self,
        entry: &LoafEntry,
        local_remapping: &HashMap<String, String>,
    ) {
        let artifact = &entry.artifact;
        let owner_route = entry.key.as_str();
        for export in &artifact.manifest.contract_metadata.identity_graph.exports {
            // A top-level export is described by the manifest's own nominal info; a nested one has to be read back
            // from its declaration, because only the declaration records the members.
            let info = if export.public_path.len() == 2 {
                self.manifest_nominal_type_info(&artifact.manifest, &export.public_name)
            } else {
                Self::api_declaration_for_target_path(&artifact.manifest, &export.source_path)
                    .and_then(|declaration| {
                        self.symbol_kind_from_api_declaration(
                            artifact.manifest.contract_metadata.api.as_ref(),
                            declaration,
                        )
                    })
                    .and_then(|kind| match kind {
                        SymbolKind::Type(info) => Some(info),
                        _ => None,
                    })
            };
            let Some(info) = info else {
                continue;
            };
            let qualified = canonical_public_library_type_name(owner_route, &export_public_name(export));
            let mut kind = SymbolKind::Type(info);
            // A member type is spelled as the declaring module spells it; the names that module reaches through a
            // private import or a submodule the root does not republish resolve through its scope first.
            let declaration = export.canonical.as_ref().and_then(|identity| identity.hydrate());
            self.remap_symbol_kind_through_declaring_module(
                owner_route,
                &artifact.manifest,
                declaration.as_ref(),
                &mut kind,
            );
            self.remap_symbol_kind_with_import_aliases(&mut kind, local_remapping);
            Self::mark_compiled_class_field_provider(&mut kind, owner_route);
            if let SymbolKind::Type(info) = kind {
                self.cache_retained_nominal_layout(declaration.as_ref(), &info);
                self.transitive_pub_types.entry(qualified).or_default().push(info);
            }
        }
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
        self.manifest_nominal_type_info(manifest, public_name)?;
        let entry = manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)?;
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
