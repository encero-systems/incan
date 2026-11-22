//! Public library function symbols, module member lookups, and transitive export semantics.

use super::deferred_layouts::DeclaringScopes;
use super::*;
use crate::library_manifest::CanonicalIdentityExport;
use crate::library_manifest::install_record::{InstallDeclarationKind, InstallExportKind, is_nominal_export};
use crate::typechecker::TransitiveLayout;
use crate::typechecker::transitive_layouts::{LayoutDeclaration, LayoutRecipe};

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
                ManifestExportRef::Alias(export) => self
                    .symbol_kind_from_manifest_alias(&manifest, export, &mut HashSet::new())
                    .or_else(|| {
                        let canonical = manifest
                            .contract_metadata
                            .identity_graph
                            .canonical_for_public_name(member);
                        self.rust_item_for_canonical(canonical.as_ref())
                    })?,
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
        if let Some(resolved) = self.record_module_member(library, module_path, member) {
            return Ok(Some(resolved));
        }
        // A name the namespace's records prove it does not publish, such as one of its child modules, is no member.
        if self.record_proves_no_member(library, module_path, member) {
            return Ok(None);
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
        // A plain function its records settle projects no partial call, and neither does a name they prove absent.
        if self.record_module_member(library, module_path, member).is_some()
            || self.record_proves_no_member(library, module_path, member)
        {
            return None;
        }
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
    /// They do not change source-visible name resolution. Each declaration's layout is deferred (see
    /// [`crate::typechecker::TransitiveLayout`]) and built the first time a lookup reads it; identities, scopes and
    /// alias layouts are installed here, in the order the tables have always been filled.
    pub(in crate::typechecker) fn cache_transitive_pub_export_semantics(
        &mut self,
        library: &str,
        manifest: &Arc<LibraryManifest>,
    ) {
        if !self.cached_pub_libraries.insert(library.to_string()) {
            return;
        }
        let canonical_remapping = Arc::new(self.public_library_canonical_type_remapping(library, manifest));
        let root_layout = |declaration, field_provider: Option<&str>| LayoutRecipe {
            manifest: Arc::clone(manifest),
            declaration,
            remappings: Vec::new(),
            field_provider: field_provider.map(str::to_owned),
        };
        let record = self.prepared_install_record(library, manifest);
        let roots = &record.root_exports;
        for (index, model) in roots.models.iter().enumerate() {
            let layout = self.deferred_layout(root_layout(LayoutDeclaration::Model(index), None));
            self.transitive_pub_types.entry(model.clone()).or_default().push(layout);
        }
        for (index, class) in roots.classes.iter().enumerate() {
            let layout = self.deferred_layout(root_layout(LayoutDeclaration::Class(index), Some(library)));
            self.transitive_pub_types.entry(class.clone()).or_default().push(layout);
        }
        for (index, enum_export) in roots.enums.iter().enumerate() {
            let layout = self.deferred_layout(root_layout(LayoutDeclaration::Enum(index), None));
            self.transitive_pub_types
                .entry(enum_export.clone())
                .or_default()
                .push(layout);
        }
        for (index, newtype) in roots.newtypes.iter().enumerate() {
            let layout = self.deferred_layout(root_layout(LayoutDeclaration::Newtype(index), None));
            self.transitive_pub_types
                .entry(newtype.clone())
                .or_default()
                .push(layout);
        }
        for (index, trait_export) in roots.traits.iter().enumerate() {
            let layout = self.deferred_layout(root_layout(LayoutDeclaration::Trait(index), None));
            self.transitive_pub_traits
                .entry(trait_export.clone())
                .or_default()
                .push(layout);
        }

        self.register_pub_library_declarations(library, manifest, &canonical_remapping);
        for declared in record.modules.iter().flat_map(|module| &module.declarations) {
            if matches!(
                declared.kind,
                InstallDeclarationKind::Model | InstallDeclarationKind::Class
            ) {
                self.record_declared_type_param_bounds(&declared.type_params, || {
                    declared.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate)
                });
            }
        }
        self.cache_canonical_type_layouts(library, manifest, &canonical_remapping);
    }

    /// Install each nominal public type of `library` under its canonical spelling and its retained carrier key, its
    /// members read in its declaring module's scope and then through `canonical_remapping`.
    ///
    /// The exports and the kind of root export each name finds are the Loaf's install record's. A model, class, enum
    /// or newtype export is deferred; an alias export is built now, because it reads the tables, and one that names no
    /// nominal type is not installed. Every layout is made before any is installed, so an alias
    /// reads the tables as the install found them.
    fn cache_canonical_type_layouts(
        &mut self,
        library: &str,
        manifest: &Arc<LibraryManifest>,
        canonical_remapping: &Arc<HashMap<String, String>>,
    ) {
        let record = self.prepared_install_record(library, manifest);
        let identity_graph = &record.identity_graph;
        let kinds = record.export_kinds_by_name();
        let mut scopes = DeclaringScopes::new();
        let mut layouts = Vec::new();
        for identity in &identity_graph.exports {
            let public_name = identity.public_name.as_str();
            let kind = kinds.get(public_name).copied().flatten();
            let (declaration, layout) = if is_nominal_export(kind) {
                let declaration = identity_graph.canonical_for_public_name(public_name);
                let scope =
                    self.declaring_module_scope(library, manifest, identity_graph, declaration.as_ref(), &mut scopes);
                let layout = self.deferred_layout(LayoutRecipe {
                    manifest: Arc::clone(manifest),
                    declaration: LayoutDeclaration::Export(public_name.to_owned()),
                    remappings: scope.into_iter().chain([Arc::clone(canonical_remapping)]).collect(),
                    field_provider: Some(library.to_owned()),
                });
                (declaration, layout)
            } else if kind == Some(InstallExportKind::Alias) {
                let Some(info) = self.manifest_nominal_type_info(manifest, public_name) else {
                    continue;
                };
                let declaration = identity_graph.canonical_for_public_name(public_name);
                let mut kind = SymbolKind::Type(info);
                self.remap_symbol_kind_through_declaring_module(library, manifest, declaration.as_ref(), &mut kind);
                self.remap_symbol_kind_with_import_aliases(&mut kind, canonical_remapping.as_ref());
                Self::mark_compiled_class_field_provider(&mut kind, library);
                let SymbolKind::Type(info) = kind else {
                    continue;
                };
                (declaration, TransitiveLayout::ready(info))
            } else {
                continue;
            };
            layouts.push((
                canonical_public_library_type_name(library, public_name),
                declaration,
                layout,
            ));
        }
        for (canonical_name, declaration, layout) in layouts {
            self.cache_retained_nominal_layout(declaration.as_ref(), &layout);
            self.transitive_pub_types
                .entry(canonical_name)
                .or_default()
                .push(layout);
        }
    }

    /// Index an admitted nominal layout by its retained identity without creating a source-visible binding. Repeated
    /// imports share the first admitted layout.
    pub(in crate::typechecker) fn cache_retained_nominal_layout(
        &mut self,
        declaration: Option<&incan_semantics_core::CanonicalSymbolId>,
        layout: &TransitiveLayout<TypeInfo>,
    ) {
        let Some(declaration) = declaration else { return };
        let Some(carrier) = incan_semantics_core::body_ir::nominal_carrier_key(declaration) else {
            return;
        };
        self.transitive_pub_types
            .entry(carrier)
            .or_insert_with(|| vec![layout.clone()]);
    }
}
