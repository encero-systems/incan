//! Manifest export lookup, symbol kinds for manifest and API targets, and public import symbols.

use super::*;

impl TypeChecker {
    /// Return all exported names in a manifest for diagnostics.
    pub(in crate::typechecker::collect) fn manifest_export_names(manifest: &LibraryManifest) -> Vec<String> {
        let mut names = Vec::new();
        names.extend(manifest.exports.models.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.aliases.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.partials.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.classes.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.functions.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.traits.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.enums.iter().map(|item| item.name.clone()));
        names.extend(
            manifest
                .exports
                .enums
                .iter()
                .flat_map(|item| item.variants.iter().map(|variant| variant.name.clone())),
        );
        names.extend(manifest.exports.type_aliases.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.newtypes.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.consts.iter().map(|item| item.name.clone()));
        names.extend(manifest.exports.statics.iter().map(|item| item.name.clone()));
        names.sort();
        names.dedup();
        names
    }

    /// Find one manifest export by name, including alias entries.
    pub(in crate::typechecker::collect) fn find_manifest_export<'a>(
        manifest: &'a LibraryManifest,
        name: &str,
    ) -> Option<ManifestExportRef<'a>> {
        if let Some(item) = manifest.exports.aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Alias(item));
        }
        if let Some(item) = manifest.exports.models.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Model(item));
        }
        if let Some(item) = manifest.exports.classes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Class(item));
        }
        if let Some(item) = manifest.exports.functions.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Function(item));
        }
        if let Some(item) = manifest.exports.partials.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Partial(item));
        }
        if let Some(item) = manifest.exports.traits.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Trait(item));
        }
        if let Some(item) = manifest.exports.enums.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Enum(item));
        }
        for enum_export in &manifest.exports.enums {
            if let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == name) {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
                });
            }
        }
        if let Some(item) = manifest.exports.type_aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::TypeAlias(item));
        }
        if let Some(item) = manifest.exports.newtypes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Newtype(item));
        }
        if let Some(item) = manifest.exports.consts.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Const(item));
        }
        if let Some(item) = manifest.exports.statics.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Static(item));
        }
        None
    }

    /// Find one manifest export by name without following alias entries.
    pub(in crate::typechecker::collect) fn find_manifest_non_alias_export<'a>(
        manifest: &'a LibraryManifest,
        name: &str,
    ) -> Option<ManifestExportRef<'a>> {
        if let Some(item) = manifest.exports.models.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Model(item));
        }
        if let Some(item) = manifest.exports.classes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Class(item));
        }
        if let Some(item) = manifest.exports.functions.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Function(item));
        }
        if let Some(item) = manifest.exports.partials.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Partial(item));
        }
        if let Some(item) = manifest.exports.traits.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Trait(item));
        }
        if let Some(item) = manifest.exports.enums.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Enum(item));
        }
        for enum_export in &manifest.exports.enums {
            if let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == name) {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
                });
            }
            if let Some(alias) = enum_export.variant_aliases.iter().find(|alias| alias.name == name)
                && let Some(variant) = enum_export.variants.iter().find(|variant| variant.name == alias.target)
            {
                return Some(ManifestExportRef::EnumVariant {
                    enum_name: &enum_export.name,
                    fields: &variant.fields,
                    canonical: variant.canonical.as_ref(),
                });
            }
        }
        if let Some(item) = manifest.exports.type_aliases.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::TypeAlias(item));
        }
        if let Some(item) = manifest.exports.newtypes.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Newtype(item));
        }
        if let Some(item) = manifest.exports.consts.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Const(item));
        }
        if let Some(item) = manifest.exports.statics.iter().find(|item| item.name == name) {
            return Some(ManifestExportRef::Static(item));
        }
        None
    }

    /// Resolve a manifest alias into the symbol it actually projects.
    ///
    /// Facades often publish aliases whose short target name is the same as the exported alias name (`Frame` →
    /// `exprs.Frame`). Resolving only by the final segment re-enters the alias export and can recurse forever. Prefer
    /// the explicit target path in checked API metadata, then fall back to non-alias manifest exports, and only follow
    /// another alias with a visited guard.
    pub(in crate::typechecker::collect) fn symbol_kind_from_manifest_alias(
        &self,
        manifest: &LibraryManifest,
        export: &AliasExport,
        visited: &mut HashSet<Vec<String>>,
    ) -> Option<SymbolKind> {
        if let Some(function) = &export.projected_function {
            return Some(SymbolKind::Function(self.function_info_from_manifest(function)));
        }
        if let Some(kind) = self.symbol_kind_from_type_projection(export.projected_type.as_ref()) {
            return Some(kind);
        }
        let identity_target_path = Self::manifest_identity_target_path(manifest, &export.name);
        let target_path = identity_target_path.unwrap_or(export.target_path.as_slice());
        if let Some(kind) = Self::symbol_kind_from_manifest_rust_target(manifest, target_path) {
            return Some(kind);
        }
        if let Some(target_path) = identity_target_path
            && !visited.contains(target_path)
            && let Some(kind) = self.symbol_kind_from_manifest_path(manifest, target_path)
        {
            return Some(kind);
        }
        if !visited.insert(export.target_path.clone()) {
            return None;
        }
        if let Some(kind) = self.symbol_kind_from_manifest_path(manifest, &export.target_path) {
            return Some(kind);
        }
        let target_name = export.target_path.last()?;
        if let Some(kind) = self
            .find_manifest_non_alias_symbol_kind(manifest, target_name)
            .or_else(|| self.pub_library_function_symbol(manifest, target_name))
        {
            return Some(kind);
        }
        let ManifestExportRef::Alias(target_alias) = Self::find_manifest_export(manifest, target_name)? else {
            return None;
        };
        self.symbol_kind_from_manifest_alias(manifest, target_alias, visited)
    }

    /// Hydrate a forwarded type or source trait from its admitted declaring artifact and checked public membership.
    ///
    /// Nominal projections prefer the reconstructed artifact layout because its recursive member types have already
    /// been remapped through their declaring module. Falling back to the raw checked API declaration is valid only
    /// before that cache exists. Trait aliases retain their generic contract without requiring a nominal layout or a
    /// native trait item.
    pub(in crate::typechecker::collect) fn symbol_kind_from_type_projection(
        &self,
        projected: Option<&crate::library_manifest::TypeRef>,
    ) -> Option<SymbolKind> {
        if projected.is_some_and(|ty| ty.has_native_union()) {
            return Some(SymbolKind::Type(TypeInfo::TypeAlias));
        }
        if let crate::library_manifest::TypeRef::Trait { identity } = projected? {
            return self.trait_symbol_kind_from_identity(identity, None);
        }
        let crate::library_manifest::TypeRef::Named {
            origin: Some(origin), ..
        } = projected?
        else {
            return None;
        };
        let canonical = origin.canonical.hydrate()?;
        if canonical.kind == SemanticSourceTargetKind::Trait {
            return self.trait_symbol_kind_from_identity(&canonical, Some(&origin.provider));
        }
        let (entry, identity) = self.loaves.public_nominal(&canonical)?;
        let artifact = &entry.artifact;
        let qualified = canonical_public_library_type_name(&entry.key, &export_public_name(identity));
        if let Some(info) = self
            .transitive_pub_types
            .get(&qualified)
            .and_then(|infos| infos.first())
        {
            return Some(SymbolKind::Type(info.clone()));
        }
        if identity.public_path.len() == 2 {
            self.manifest_nominal_type_info(&artifact.manifest, &identity.public_name)
                .map(SymbolKind::Type)
        } else {
            Self::api_declaration_for_target_path(&artifact.manifest, &identity.source_path).and_then(|declaration| {
                self.symbol_kind_from_api_declaration(artifact.manifest.contract_metadata.api.as_ref(), declaration)
            })
        }
    }

    /// Reconstruct a declaration from the compiled artifact that owns its canonical identity.
    ///
    /// A cross-package facade alias may not carry a callable or nominal projection because the target is a const,
    /// static or enum variant. The identity graph still names the exact declaring artifact and source declaration, so
    /// consumers hydrate that ordinary declaration instead of treating the facade member as ambiguous. Hydration
    /// admits the declaring Loaf through the normal import boundary so its ownership contracts accompany the symbol.
    pub(in crate::typechecker::collect) fn symbol_kind_from_canonical_identity(
        &mut self,
        canonical: &CanonicalSymbolId,
    ) -> Option<SymbolKind> {
        let library = self.loaves.declaring_loaf(canonical)?.key.clone();
        let manifest = self.loaf_manifest_or_refuse(&library, Span::default())?;
        let identity = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .find(|identity| {
                identity
                    .canonical
                    .as_ref()
                    .is_some_and(|value| value.represents(canonical))
            })?;
        let declaration = Self::api_declaration_for_target_path(&manifest, &identity.source_path)?;
        let mut kind = self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration)?;
        let root_remapping = self.public_library_canonical_type_remapping(&library, &manifest);
        self.remap_symbol_kind_through_declaring_module(&library, &manifest, Some(canonical), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &root_remapping);
        Some(kind)
    }

    /// Reconstruct one trait from the exact canonical identity admitted by its declaring manifest.
    ///
    /// `expected_provider` retains the older named-projection check that the alias metadata and selected artifact name
    /// the same provider. Native trait projections already carry the canonical identity directly and need no route
    /// provenance check.
    fn trait_symbol_kind_from_identity(
        &self,
        canonical: &CanonicalSymbolId,
        expected_provider: Option<&crate::loaf_index::LoafIdentity>,
    ) -> Option<SymbolKind> {
        let entry = self.loaves.declaring_loaf(canonical)?;
        if expected_provider.is_some_and(|provider| &entry.artifact.identity != provider) {
            return None;
        }
        let manifest = &entry.artifact.manifest;
        let identity = manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .find(|identity| {
                identity.kind == crate::library_manifest::ExportIdentityKind::Trait
                    && identity
                        .canonical
                        .as_ref()
                        .is_some_and(|value| value.represents(canonical))
            })?;
        let kind = if identity.public_path.len() == 2 {
            manifest
                .exports
                .traits
                .iter()
                .find(|item| item.name == identity.public_name)
                .map(|item| SymbolKind::Trait(self.trait_info_from_manifest(item)))
        } else {
            Self::api_declaration_for_target_path(manifest, &identity.source_path).and_then(|declaration| {
                self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration)
            })
        };
        kind.filter(|kind| matches!(kind, SymbolKind::Trait(_)))
    }

    /// Reconstruct a checked `rust::` reexport from the ABI metadata shipped beside the public manifest.
    pub(in crate::typechecker::collect) fn symbol_kind_from_manifest_rust_target(
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        let kind = Self::rust_reexport_symbol_kind(manifest, target_path)?;
        matches!(&kind, SymbolKind::RustItem(info) if info.metadata.is_some()).then_some(kind)
    }

    /// The Rust item a provider's public `rust::` reexport names, bound as the provider's own import is. Metadata the
    /// provider shipped travels with it; otherwise the consumer's checker inspects the item by its path on demand.
    pub(in crate::typechecker::collect) fn rust_reexport_symbol_kind(
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        let [namespace, crate_name, rest @ ..] = target_path else {
            return None;
        };
        if namespace != "rust" || rest.is_empty() {
            return None;
        }
        let path = target_path[1..].join("::");
        let metadata = manifest
            .rust_abi
            .as_ref()
            .and_then(|abi| abi.get(&path))
            .map(|metadata| Arc::new(metadata.clone()));
        Some(SymbolKind::RustItem(RustItemInfo {
            crate_name: crate_name.clone(),
            metadata,
            path,
            binding: RustImportBindingKind::FromImport,
        }))
    }

    /// Return the canonical target path published by the manifest identity graph for one public name.
    pub(in crate::typechecker::collect) fn manifest_identity_target_path<'a>(
        manifest: &'a LibraryManifest,
        public_name: &str,
    ) -> Option<&'a [String]> {
        manifest
            .contract_metadata
            .identity_graph
            .entry_for_public_name(public_name)
            .and_then(|entry| entry.target_path())
    }

    /// Resolve an exact manifest identity path into a semantic symbol kind before falling back to final-segment lookup.
    pub(in crate::typechecker::collect) fn symbol_kind_from_manifest_path(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        if let Some(kind) = self.symbol_kind_from_api_target_path(manifest, target_path) {
            return Some(kind);
        }
        let target_name = target_path.last()?;
        self.find_manifest_non_alias_symbol_kind(manifest, target_name)
            .or_else(|| self.pub_library_function_symbol(manifest, target_name))
    }

    /// Convert one non-alias manifest export into checker symbol metadata.
    pub(in crate::typechecker::collect) fn find_manifest_non_alias_symbol_kind(
        &self,
        manifest: &LibraryManifest,
        name: &str,
    ) -> Option<SymbolKind> {
        match Self::find_manifest_non_alias_export(manifest, name)? {
            ManifestExportRef::Model(export) => {
                Some(SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export))))
            }
            ManifestExportRef::Class(export) => {
                Some(SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export))))
            }
            ManifestExportRef::Function(export) => Some(SymbolKind::Function(self.function_info_from_manifest(export))),
            ManifestExportRef::Partial(export) => {
                Some(SymbolKind::Function(self.partial_info_from_manifest(manifest, export)))
            }
            ManifestExportRef::Trait(export) => Some(SymbolKind::Trait(self.trait_info_from_manifest(export))),
            ManifestExportRef::Enum(export) => {
                Some(SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))))
            }
            ManifestExportRef::EnumVariant {
                enum_name,
                fields,
                canonical,
            } => Some(SymbolKind::Variant(VariantInfo {
                identity: canonical.and_then(|identity| identity.hydrate()),
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            })),
            ManifestExportRef::TypeAlias(_) => Some(SymbolKind::Type(TypeInfo::TypeAlias)),
            ManifestExportRef::Newtype(export) => Some(SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(export),
            ))),
            ManifestExportRef::Const(export) => Some(SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_mutable: false,
                is_used: false,
            })),
            ManifestExportRef::Static(export) => Some(SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&export.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            })),
            ManifestExportRef::Alias(_) => None,
        }
    }

    /// Follow a published alias's retained declaration identity before its written target. Standard-library facades
    /// may spell a same-package target as `std.io.IoError`, while the package API stores its declaration at
    /// `io.IoError`. Foreign projections without a declaration in this package retain their admitted target path.
    pub(in crate::typechecker::collect) fn api_alias_target_path<'a>(
        manifest: &'a LibraryManifest,
        module_path: &[String],
        name: &str,
        target_path: &'a [String],
    ) -> &'a [String] {
        let public_path = std::iter::once(manifest.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(name.to_string()))
            .collect::<Vec<_>>();
        let identities = &manifest.contract_metadata.identity_graph;
        identities
            .canonical_for_public_path(&public_path)
            .and_then(|canonical| identities.declaration_source_path(&canonical))
            .unwrap_or(target_path)
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    pub(in crate::typechecker::collect) fn api_declaration_for_target_path<'a>(
        manifest: &'a LibraryManifest,
        target_path: &[String],
    ) -> Option<&'a ApiDeclaration> {
        let name = target_path.last()?;
        let module_path = if target_path.first().is_some_and(|segment| segment == "crate") {
            &target_path[1..]
        } else {
            target_path
        };
        let module_path = module_path.get(..module_path.len().saturating_sub(1))?;
        let api = manifest.contract_metadata.api.as_ref()?;
        let module = api.modules.iter().find(|module| module.module_path == module_path)?;
        module.declarations.iter().find(|declaration| match declaration {
            ApiDeclaration::Function(item) => item.name == *name,
            ApiDeclaration::Model(item) => item.name == *name,
            ApiDeclaration::Class(item) => item.name == *name,
            ApiDeclaration::Trait(item) => item.name == *name,
            ApiDeclaration::Enum(item) => item.name == *name,
            ApiDeclaration::Newtype(item) => item.name == *name,
            ApiDeclaration::TypeAlias(item) => item.name == *name,
            ApiDeclaration::Const(item) => item.name == *name,
            ApiDeclaration::Static(item) => item.name == *name,
            ApiDeclaration::Alias(item) => item.name == *name,
            ApiDeclaration::Partial(item) => item.name == *name,
        })
    }

    /// Normalize one API declaration target into its authored module path and declaration name.
    pub(in crate::typechecker::collect) fn normalized_api_declaration_target(
        target_path: &[String],
    ) -> Option<(Vec<String>, String)> {
        let path = if target_path.first().is_some_and(|segment| segment == "crate") {
            &target_path[1..]
        } else {
            target_path
        };
        let (name, module_path) = path.split_last()?;
        Some((module_path.to_vec(), name.clone()))
    }

    /// Resolve an alias target path against the checked API metadata embedded in the manifest.
    pub(in crate::typechecker::collect) fn symbol_kind_from_api_target_path(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
    ) -> Option<SymbolKind> {
        self.symbol_kind_from_api_target_path_inner(manifest, target_path, &mut HashSet::new())
    }

    /// Follow checked API alias targets while refusing cycles.
    pub(in crate::typechecker::collect) fn symbol_kind_from_api_target_path_inner(
        &self,
        manifest: &LibraryManifest,
        target_path: &[String],
        visited: &mut HashSet<Vec<String>>,
    ) -> Option<SymbolKind> {
        if !visited.insert(target_path.to_vec()) {
            return None;
        }
        // A re-exported Rust item binds as the Rust item its provider imported.
        if let Some(kind) = Self::rust_reexport_symbol_kind(manifest, target_path) {
            return Some(kind);
        }
        let declaration = Self::api_declaration_for_target_path(manifest, target_path)?;
        match declaration {
            ApiDeclaration::Alias(alias) => {
                if let Some(function) = &alias.projected_function {
                    return Some(SymbolKind::Function(
                        self.function_info_from_manifest(&function_export_from_api_projected(function)),
                    ));
                }
                self.symbol_kind_from_type_projection(alias.projected_type.as_ref())
                    .or_else(|| self.symbol_kind_from_api_target_path_inner(manifest, &alias.target_path, visited))
            }
            _ => self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration),
        }
    }

    /// Convert one checked API declaration into the same semantic symbols used for manifest exports.
    ///
    /// `api` is the checked API the declaration belongs to; a partial's leftover defaulted parameters are completed
    /// from its target there (#1760).
    pub(in crate::typechecker::collect) fn symbol_kind_from_api_declaration(
        &self,
        api: Option<&CheckedApiMetadataPackage>,
        declaration: &ApiDeclaration,
    ) -> Option<SymbolKind> {
        match declaration {
            ApiDeclaration::Function(item) => Some(SymbolKind::Function(
                self.function_info_from_manifest(&function_export_from_api(item)),
            )),
            ApiDeclaration::Model(item) => Some(SymbolKind::Type(TypeInfo::Model(
                self.model_info_from_manifest(&model_export_from_api(item)),
            ))),
            ApiDeclaration::Class(item) => Some(SymbolKind::Type(TypeInfo::Class(
                self.class_info_from_manifest(&class_export_from_api(item)),
            ))),
            ApiDeclaration::Trait(item) => Some(SymbolKind::Trait(
                self.trait_info_from_manifest(&trait_export_from_api(item)),
            )),
            ApiDeclaration::Enum(item) => Some(SymbolKind::Type(TypeInfo::Enum(
                self.enum_info_from_manifest(&enum_export_from_api(item)),
            ))),
            ApiDeclaration::Newtype(item) => Some(SymbolKind::Type(TypeInfo::Newtype(
                self.newtype_info_from_manifest(&newtype_export_from_api(item)),
            ))),
            ApiDeclaration::TypeAlias(_) => Some(SymbolKind::Type(TypeInfo::TypeAlias)),
            ApiDeclaration::Const(item) => Some(SymbolKind::Variable(VariableInfo {
                ty: resolved_type_from_manifest_type_ref(&item.ty),
                is_mutable: false,
                is_used: false,
            })),
            ApiDeclaration::Static(item) => Some(SymbolKind::Static(StaticInfo {
                ty: resolved_type_from_manifest_type_ref(&item.ty),
                is_public: true,
                is_imported: true,
                is_used: false,
            })),
            ApiDeclaration::Alias(item) => item
                .projected_function
                .as_ref()
                .map(|function| {
                    SymbolKind::Function(
                        self.function_info_from_manifest(&function_export_from_api_projected(function)),
                    )
                })
                .or_else(|| self.symbol_kind_from_type_projection(item.projected_type.as_ref())),
            ApiDeclaration::Partial(item) => {
                Some(SymbolKind::Function(self.partial_info_from_export(&api.map_or_else(
                    || partial_export_from_api(item),
                    |api| partial_export_with_target_defaults(api, item),
                ))))
            }
        }
    }

    /// Return whether a manifest export introduces a type-like name into the importing module.
    pub(in crate::typechecker::collect) fn manifest_export_is_type(export: &ManifestExportRef<'_>) -> bool {
        matches!(export, ManifestExportRef::Alias(alias) if alias.projected_type.is_some())
            || matches!(
                export,
                ManifestExportRef::Model(_)
                    | ManifestExportRef::Class(_)
                    | ManifestExportRef::Trait(_)
                    | ManifestExportRef::Enum(_)
                    | ManifestExportRef::TypeAlias(_)
                    | ManifestExportRef::Newtype(_)
            )
    }

    /// Define one symbol imported from a public library manifest.
    #[allow(clippy::too_many_arguments)] // Keeps provider, projection, alias, and source evidence explicit.
    pub(in crate::typechecker::collect) fn define_pub_import_symbol(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        source_name: &str,
        local_name: String,
        export: ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let partial_projection =
            self.partial_projection_from_manifest_export(manifest, &local_name, &export, imported_type_aliases, span);
        let declared_type_params = match &export {
            ManifestExportRef::Model(model) => model.type_params.clone(),
            ManifestExportRef::Class(class) => class.type_params.clone(),
            _ => Vec::new(),
        };
        let Some((mut kind, type_alias_target)) =
            self.symbol_kind_for_manifest_export(manifest, export, imported_type_aliases)
        else {
            return;
        };
        let canonical = manifest
            .contract_metadata
            .identity_graph
            .canonical_for_public_name(source_name);
        self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);
        Self::mark_compiled_class_field_provider(&mut kind, library);

        let mut binding_path = self.loaf_binding_root(library);
        binding_path.push(source_name.to_string());
        if let SymbolKind::RustItem(info) = kind {
            self.symbols.define_import_binding_with_inferred_target_at_path(
                Symbol {
                    name: local_name,
                    kind: SymbolKind::RustItem(info),
                    span,
                    scope: 0,
                },
                binding_path,
            );
            return;
        }
        let symbol_id = self.symbols.define_import_binding_at_path(
            Symbol {
                name: local_name.clone(),
                kind: kind.clone(),
                span,
                scope: 0,
            },
            canonical.clone(),
            binding_path,
        );
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        if let Some(identity) = canonical {
            self.type_info
                .declarations
                .resolved_import_identities
                .insert(local_name.clone(), identity);
        }
        if let Some(identity) = self.public_library_nominal_type_identity(library, manifest, source_name) {
            self.public_library_type_identities.insert(local_name.clone(), identity);
        }
        self.record_manifest_type_param_bounds(&local_name, &declared_type_params);
        if let Some(target) = type_alias_target {
            self.record_dependency_import_type_alias_before_change(&local_name);
            self.type_aliases.insert(local_name.clone(), target);
        }
        self.record_imported_function_binding(&local_name, &kind);
        if let Some(projection) = partial_projection {
            self.type_info.record_partial_projection(projection);
        }
    }

    /// Reconstruct an imported symbol from its checked manifest export before publishing a local binding. Alias
    /// targets retain their projected native union or type binders; an unresolved alias does not create a symbol.
    pub(super) fn symbol_kind_for_manifest_export(
        &mut self,
        manifest: &LibraryManifest,
        export: ManifestExportRef<'_>,
        imported_type_aliases: &HashMap<String, String>,
    ) -> Option<(SymbolKind, Option<crate::typechecker::TypeAliasTarget>)> {
        let mut type_alias_target = None;
        let kind = match export {
            ManifestExportRef::Model(export) => {
                SymbolKind::Type(TypeInfo::Model(self.model_info_from_manifest(export)))
            }
            ManifestExportRef::Class(export) => {
                SymbolKind::Type(TypeInfo::Class(self.class_info_from_manifest(export)))
            }
            ManifestExportRef::Function(export) => SymbolKind::Function(self.function_info_from_manifest(export)),
            ManifestExportRef::Partial(export) => {
                SymbolKind::Function(self.partial_info_from_manifest(manifest, export))
            }
            ManifestExportRef::Trait(export) => SymbolKind::Trait(self.trait_info_from_manifest(export)),
            ManifestExportRef::Enum(export) => SymbolKind::Type(TypeInfo::Enum(self.enum_info_from_manifest(export))),
            ManifestExportRef::EnumVariant {
                enum_name,
                fields,
                canonical,
            } => SymbolKind::Variant(VariantInfo {
                identity: canonical.and_then(|identity| identity.hydrate()),
                enum_name: enum_name.to_string(),
                fields: fields.iter().map(resolved_type_from_manifest_type_ref).collect(),
            }),
            ManifestExportRef::TypeAlias(export) => {
                let mut target = resolved_type_from_manifest_type_ref(&export.target);
                Self::remap_resolved_type_with_import_aliases(&mut target, imported_type_aliases);
                type_alias_target = Some(crate::typechecker::TypeAliasTarget {
                    type_params: export.type_params.iter().map(|param| param.name.clone()).collect(),
                    target,
                    nominal_identities: Default::default(),
                });
                SymbolKind::Type(TypeInfo::TypeAlias)
            }
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
            ManifestExportRef::Alias(export) => {
                if let Some(projected) = export.projected_type.as_ref().filter(|ty| ty.has_native_union()) {
                    let mut target = resolved_type_from_manifest_type_ref(projected);
                    Self::remap_resolved_type_with_import_aliases(&mut target, imported_type_aliases);
                    type_alias_target = Some(crate::typechecker::TypeAliasTarget {
                        type_params: Vec::new(),
                        target,
                        nominal_identities: Default::default(),
                    });
                }
                self.symbol_kind_from_manifest_alias(manifest, export, &mut HashSet::new())?
            }
        };
        Some((kind, type_alias_target))
    }

    /// Attach the importing dependency key to every field reconstructed from one compiled class manifest.
    pub(in crate::typechecker::collect) fn mark_compiled_class_field_provider(kind: &mut SymbolKind, library: &str) {
        let SymbolKind::Type(TypeInfo::Class(info)) = kind else {
            return;
        };
        for name in &info.field_order {
            info.field_provider_libraries.insert(name.clone(), library.to_string());
        }
    }
}
