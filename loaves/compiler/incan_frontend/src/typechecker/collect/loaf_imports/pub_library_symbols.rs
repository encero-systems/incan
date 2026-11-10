//! Public library function symbols, module member lookups, and transitive export semantics.

use super::*;

impl TypeChecker {
    /// Resolve one exported function name from a public manifest into a local symbol kind.
    pub(in crate::typechecker) fn pub_library_function_symbol(
        &self,
        manifest: &LibraryManifest,
        member: &str,
    ) -> Option<SymbolKind> {
        let functions = manifest
            .exports
            .functions
            .iter()
            .filter(|item| item.name == member)
            .collect::<Vec<_>>();
        match functions.as_slice() {
            [] => None,
            [function] => Some(SymbolKind::Function(self.function_info_from_manifest(function))),
            _ => {
                let identities = manifest
                    .contract_metadata
                    .identity_graph
                    .function_identities_for_public_name(member);
                Some(SymbolKind::FunctionOverloads(
                    functions
                        .into_iter()
                        .enumerate()
                        .map(|(index, function)| FunctionOverloadInfo {
                            info: self.function_info_from_manifest(function),
                            span: Span::default(),
                            identity: identities.get(index).cloned().flatten(),
                        })
                        .collect(),
                ))
            }
        }
    }

    /// Resolve one exported member from an imported `pub::` library as a symbol kind.
    ///
    /// This is used by qualified alias collection, where the import remains a module binding (`lib.member`) instead of
    /// a direct `from pub::lib import member` symbol. Alias exports are followed to their manifest target before the
    /// projected kind is returned.
    pub(in crate::typechecker) fn lookup_pub_library_symbol_member(
        &mut self,
        library: &str,
        member: &str,
    ) -> Option<SymbolKind> {
        let manifest = std::sync::Arc::clone(&self.loaves.entry(library)?.artifact.manifest);
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let remapping = self.public_library_canonical_type_remapping(library, &manifest);
        let mut kind = if let Some(kind) = self.pub_library_function_symbol(&manifest, member) {
            kind
        } else {
            let export = Self::find_manifest_export(&manifest, member)?;
            match export {
                ManifestExportRef::Model(export) => {
                    SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
                }
                ManifestExportRef::Class(export) => {
                    SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
                }
                ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
                ManifestExportRef::Partial(export) => {
                    SymbolKind::Function(self.partial_info_from_manifest(&manifest, export))
                }
                ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
                ManifestExportRef::Enum(export) => {
                    SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export)))
                }
                ManifestExportRef::TypeAlias(_) => SymbolKind::Type(TypeInfo::TypeAlias),
                ManifestExportRef::Newtype(export) => {
                    SymbolKind::Type(TypeInfo::Newtype(self.newtype_info_from_manifest(export)))
                }
                ManifestExportRef::Const(export) => SymbolKind::Variable(VariableInfo {
                    ty: resolved_type_from_manifest_type_ref(&export.ty),
                    is_mutable: false,
                    is_used: false,
                }),
                ManifestExportRef::Static(export) => SymbolKind::Static(StaticInfo {
                    ty: resolved_type_from_manifest_type_ref(&export.ty),
                    is_public: true,
                    is_imported: true,
                    is_used: false,
                }),
                ManifestExportRef::EnumVariant {
                    enum_name,
                    fields,
                    canonical,
                } => SymbolKind::Variant(VariantInfo {
                    identity: canonical.and_then(|identity| identity.hydrate()),
                    enum_name: enum_name.to_string(),
                    fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
                }),
                ManifestExportRef::Alias(export) => {
                    self.symbol_kind_from_manifest_alias(&manifest, export, &mut HashSet::new())?
                }
            }
        };
        let canonical = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(member);
        self.remap_symbol_kind_through_declaring_module(library, &manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        Self::mark_compiled_class_field_provider(&mut kind, library);
        Some(kind)
    }

    /// Resolve a member through a checked public module namespace and return its authored source module path.
    pub(in crate::typechecker) fn lookup_pub_library_module_symbol_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Option<(SymbolKind, Vec<String>)> {
        self.resolve_pub_library_module_symbol_member(library, module_path, member)
            .ok()
            .flatten()
            .map(|resolved| (resolved.kind, resolved.source_module_path))
    }

    /// Resolve a checked public module member while preserving sibling ambiguity for caller-owned diagnostics.
    pub(in crate::typechecker) fn resolve_pub_library_module_symbol_member(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
    ) -> Result<Option<ResolvedPublicModuleSymbol>, Vec<String>> {
        if module_path.is_empty() {
            let Some(kind) = self.lookup_pub_library_symbol_member(library, member) else {
                return Ok(None);
            };
            let canonical = self.loaves.manifest(library).and_then(|manifest| {
                manifest
                    .contract_metadata
                    .identity_graph
                    .canonical_for_public_name(member)
            });
            return Ok(Some(ResolvedPublicModuleSymbol {
                kind,
                canonical,
                source_module_path: self.loaf_binding_root(library),
                source_name: member.to_string(),
            }));
        }
        let Some(manifest) = self
            .loaves
            .entry(library)
            .map(|entry| std::sync::Arc::clone(&entry.artifact.manifest))
        else {
            return Ok(None);
        };
        let Some(resolved) = self.public_module_member(library, &manifest, module_path, member)? else {
            return Ok(None);
        };
        let mut source_module_path = self.loaf_binding_root(library);
        source_module_path.extend(resolved.source_module_path);
        Ok(Some(ResolvedPublicModuleSymbol {
            kind: resolved.kind,
            canonical: resolved.canonical,
            source_module_path,
            source_name: resolved.source_name,
        }))
    }

    /// Resolve partial-call projection metadata for a member reached through a checked public module namespace.
    pub(in crate::typechecker) fn lookup_pub_library_module_partial_projection(
        &mut self,
        library: &str,
        module_path: &[String],
        member: &str,
        binding_name: &str,
    ) -> Option<PartialProjectionInfo> {
        let manifest = std::sync::Arc::clone(&self.loaves.entry(library)?.artifact.manifest);
        let mut projection = self
            .public_module_member(library, &manifest, module_path, member)
            .ok()
            .flatten()?
            .partial_projection?;
        projection.name = binding_name.to_string();
        Some(projection)
    }

    /// Seed internal semantic caches for one `pub::` library's exported types and traits.
    ///
    /// These caches are used only by the consumer-side typechecker when imported signatures mention provider types
    /// that the consumer did not explicitly import by name (for example `Session.read_csv(...) -> LazyFrame[T]`).
    /// They do not change source-visible name resolution.
    pub(in crate::typechecker) fn cache_transitive_pub_export_semantics(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
    ) {
        if !self.cached_pub_libraries.insert(library.to_string()) {
            return;
        }
        let canonical_remapping = self.public_library_canonical_type_remapping(library, manifest);

        for model in &manifest.exports.models {
            let model_info = self.model_info_from_manifest(model);
            self.transitive_pub_types
                .entry(model.name.clone())
                .or_default()
                .push(TypeInfo::Model(model_info));
        }
        for class in &manifest.exports.classes {
            let mut kind = SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(class)));
            Self::mark_compiled_class_field_provider(&mut kind, library);
            if let SymbolKind::Type(info) = kind {
                self.transitive_pub_types
                    .entry(class.name.clone())
                    .or_default()
                    .push(info);
            }
        }
        for enum_export in &manifest.exports.enums {
            let enum_info = self.enum_info_from_manifest(enum_export);
            self.transitive_pub_types
                .entry(enum_export.name.clone())
                .or_default()
                .push(TypeInfo::Enum(enum_info));
        }
        for newtype in &manifest.exports.newtypes {
            let newtype_info = self.newtype_info_from_manifest(newtype);
            self.transitive_pub_types
                .entry(newtype.name.clone())
                .or_default()
                .push(TypeInfo::Newtype(newtype_info));
        }
        for trait_export in &manifest.exports.traits {
            let trait_info = self.trait_info_from_manifest(trait_export);
            self.transitive_pub_traits
                .entry(trait_export.name.clone())
                .or_default()
                .push(trait_info);
        }

        self.register_pub_library_declarations(library, manifest, &canonical_remapping);
        if let Some(api) = manifest.contract_metadata.api.as_ref() {
            for module in &api.modules {
                for declaration in &module.declarations {
                    let public_path = std::iter::once(manifest.name.clone())
                        .chain(module.module_path.iter().cloned())
                        .chain(std::iter::once(Self::api_declaration_name(declaration).to_string()))
                        .collect::<Vec<_>>();
                    let canonical = manifest
                        .contract_metadata
                        .identity_graph
                        .canonical_for_public_path(&public_path);
                    self.record_provider_type_param_bounds(declaration, canonical.as_ref());
                }
            }
        }
        let identity_graph = &manifest.contract_metadata.identity_graph;
        let canonical_types = identity_graph
            .exports
            .iter()
            .filter_map(|identity| {
                let info = self.manifest_nominal_type_info(manifest, &identity.public_name)?;
                Some((
                    canonical_public_library_type_name(library, &identity.public_name),
                    identity_graph.canonical_for_public_name(&identity.public_name),
                    info,
                ))
            })
            .collect::<Vec<_>>();
        for (canonical_name, declaration, info) in canonical_types {
            let mut kind = SymbolKind::Type(info);
            self.remap_symbol_kind_through_declaring_module(library, manifest, declaration.as_ref(), &mut kind);
            self.remap_symbol_kind_with_import_aliases(&mut kind, &canonical_remapping);
            Self::mark_compiled_class_field_provider(&mut kind, library);
            if let SymbolKind::Type(info) = kind {
                self.cache_retained_nominal_layout(declaration.as_ref(), &info);
                self.transitive_pub_types.entry(canonical_name).or_default().push(info);
            }
        }
    }

    /// Index an admitted nominal layout by its retained identity without creating a source-visible binding. Repeated
    /// imports share the first admitted layout.
    pub(in crate::typechecker) fn cache_retained_nominal_layout(
        &mut self,
        declaration: Option<&incan_semantics_core::CanonicalSymbolId>,
        info: &TypeInfo,
    ) {
        let Some(declaration) = declaration else { return };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(declaration) else {
            return;
        };
        self.transitive_pub_types
            .entry(carrier)
            .or_insert_with(|| vec![info.clone()]);
    }
}
