//! Standard Loaf traits, registry types, binding roots and standard Loaf imports.

use super::*;
impl TypeChecker {
    /// Materialize one otherwise-unbound value exported by the source-defined standard prelude.
    ///
    /// The prelude declaration, not a compiler spelling table, decides which values are ambient. Existing bindings
    /// always win, so parameters, imports and local declarations retain ordinary lexical shadowing.
    pub(in crate::typechecker) fn materialize_implicit_prelude_value(&mut self, name: &str, span: Span) -> bool {
        if name.contains('.') || self.symbols.lookup(name).is_some() {
            return false;
        }
        let module = vec!["std".to_string(), "prelude".to_string()];
        let Some((library, path)) = self.loaf_module(&module) else {
            return false;
        };
        let Some(resolved) = self
            .resolve_pub_library_module_symbol_member(&library, &path, name)
            .ok()
            .flatten()
        else {
            return false;
        };
        if !matches!(
            resolved.kind,
            SymbolKind::Variable(_)
                | SymbolKind::Static(_)
                | SymbolKind::Function(_)
                | SymbolKind::FunctionOverloads(_)
                | SymbolKind::Variant(_)
        ) {
            return false;
        }
        self.collect_loaf_from_imports(
            &LoafImport {
                key: library,
                module: path,
            },
            &[ImportItem {
                name: name.to_string(),
                alias: None,
            }],
            span,
        );
        self.symbols.lookup(name).is_some()
    }

    /// The trait a Loaf publishes as `<module_path>.<name>`, read from its manifest.
    pub(in crate::typechecker) fn loaf_trait(&mut self, module_path: &[String], name: &str) -> Option<TraitInfo> {
        let (library, rest) = self.loaf_module(module_path)?;
        match self.lookup_pub_library_module_symbol_member(&library, &rest, name)?.0 {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }

    /// The Rust derive paths a Loaf trait declares (`@rust.derive(...)`), in declaration order.
    pub(in crate::typechecker) fn loaf_trait_rust_derive_paths(
        &self,
        module_path: &[String],
        name: &str,
    ) -> Option<Vec<String>> {
        let (library, _) = self.loaf_module(module_path)?;
        let manifest = self.loaves.manifest(&library)?;
        let canonical = self.loaf_member_identity(module_path, name)?;
        let source_path = manifest
            .contract_metadata
            .identity_graph
            .declaration_source_path(&canonical)?;
        let ApiDeclaration::Trait(declaration) = Self::api_declaration_for_target_path(manifest, source_path)? else {
            return None;
        };
        let mut paths = Vec::new();
        for decorator in &declaration.decorators {
            if core_decorators::from_str(&decorator.path.join(".")) != Some(DecoratorId::RustDerive) {
                continue;
            }
            for argument in &decorator.args {
                if let DecoratorArgMetadata::Positional {
                    value:
                        DecoratorValue::Literal {
                            value: SafeMetadataValue::String(path),
                        },
                } = argument
                    && !paths.contains(path)
                {
                    paths.push(path.clone());
                }
            }
        }
        Some(paths)
    }

    /// The RFC 024 `__derives__` trait list a Loaf module publishes in its checked API metadata.
    pub(in crate::typechecker) fn loaf_module_derivable_traits(&self, module_path: &[String]) -> Option<Vec<String>> {
        let (library, rest) = self.loaf_module(module_path)?;
        let api = self.loaves.manifest(&library)?.contract_metadata.api.as_ref()?;
        api.modules
            .iter()
            .find(|module| module.module_path == rest)
            .map(|module| module.derivable_traits.clone())
    }

    /// Record the standard `Registry` declarations when this module defines, imports or enters a registry, so lowering
    /// selects the registry's reserved methods from the checked declaration.
    pub(in crate::typechecker) fn record_standard_registry_types(&mut self) {
        let registry = &self.type_info.registry;
        if registry.definitions.is_empty()
            && registry.imported_definitions.is_empty()
            && registry.explicit_entries.is_empty()
        {
            return;
        }
        let module = vec![stdlib::STDLIB_ROOT.to_string(), stdlib::STDLIB_REGISTRY.to_string()];
        let standard = if self.owns_standard_module(&module) {
            self.own_standard_registry_types(&module)
        } else {
            self.loaf_standard_registry_types(&module)
        };
        if let Some(standard) = standard {
            self.type_info.registry.standard = Some(standard);
        }
    }

    /// Whether the active binding denotes this exact Loaf declaration or its registry-owned builtin placeholder.
    /// A placeholder is compatible with its declaring module, but still needs that module's checked contract.
    pub(super) fn rebinds_same_declaration(&self, local_name: &str, module: &[String], item: &str) -> bool {
        let Some(symbol) = self.symbols.lookup_local(local_name) else {
            return false;
        };
        match self.symbols.identity_of(symbol) {
            Some(existing) if !matches!(existing.origin, SymbolOrigin::Builtin) => {
                self.loaf_member_identity(module, item).as_ref() == Some(existing)
            }
            // A compiler-owned placeholder may be imported through a facade only when both exports name the
            // registered declaring module's exact trait. A matching short name grants no declaration authority.
            Some(existing) if existing.origin == SymbolOrigin::Builtin => {
                if local_name != item
                    || !matches!(
                        self.symbols.get(symbol).map(|found| &found.kind),
                        Some(SymbolKind::Trait(_))
                    )
                {
                    return false;
                }
                let Some(declaring) =
                    incan_lang::lang::traits::from_str(item).and_then(incan_lang::lang::traits::source_module)
                else {
                    return false;
                };
                let declaring = declaring.split('.').map(str::to_owned).collect::<Vec<_>>();
                let Some(imported) = self.loaf_member_identity(module, item) else {
                    return false;
                };
                self.loaf_member_identity(&declaring, item).as_ref() == Some(&imported)
            }
            _ => false,
        }
    }

    /// Keep real imports idempotent while replacing a compatible builtin placeholder with checked Loaf metadata.
    /// A bare spelling is never sufficient: only the builtin registry's declaring module can supply its contract.
    pub(super) fn loaf_import_needs_binding(
        &mut self,
        local_name: &str,
        module: &[String],
        item: &str,
        span: Span,
    ) -> bool {
        let Some(existing_kind) = self.existing_local_symbol_kind(local_name) else {
            return true;
        };
        if self.rebinds_same_declaration(local_name, module, item) {
            return self
                .symbols
                .lookup_local(local_name)
                .and_then(|symbol| self.symbols.identity_of(symbol))
                .is_some_and(|identity| matches!(identity.origin, SymbolOrigin::Builtin));
        }
        self.errors.push(errors::pub_library_import_name_collision(
            local_name,
            existing_kind,
            span,
        ));
        false
    }

    /// The standard `Registry` declarations of the package that declares `std.registry`: its own checked modules.
    pub(super) fn own_standard_registry_types(
        &self,
        module: &[String],
    ) -> Option<crate::typechecker::StandardRegistryTypes> {
        let path = ImportPath {
            parent_levels: 0,
            is_absolute: false,
            segments: module.to_vec(),
        };
        let Some(SymbolKind::Type(TypeInfo::Class(registry_class))) =
            self.dependency_member_symbol_for_path(&path, "Registry")
        else {
            return None;
        };
        Some(crate::typechecker::StandardRegistryTypes {
            registry: self.dependency_member_identity(&path, "Registry")?,
            registry_class,
            entry: self.dependency_member_identity(&path, "RegistryEntry")?,
        })
    }

    /// The standard `Registry` declarations, read from the standard Loaf that owns `std.registry`.
    pub(super) fn loaf_standard_registry_types(
        &mut self,
        module: &[String],
    ) -> Option<crate::typechecker::StandardRegistryTypes> {
        let (library, rest) = self.loaf_module(module)?;
        let Some((SymbolKind::Type(TypeInfo::Class(registry_class)), _)) =
            self.lookup_pub_library_module_symbol_member(&library, &rest, "Registry")
        else {
            return None;
        };
        Some(crate::typechecker::StandardRegistryTypes {
            registry: self.loaf_member_identity(module, "Registry")?,
            registry_class,
            entry: self.loaf_member_identity(module, "RegistryEntry")?,
        })
    }

    /// The standard declaration of the builtin trait `name` (`Eq`, `Ord`, …), read once from the standard package
    /// owning its module.
    pub(in crate::typechecker) fn standard_trait(&mut self, name: &str) -> Option<TraitInfo> {
        if let Some(info) = self.standard_traits.get(name) {
            return Some(info.clone());
        }
        // The derive-method table names a derivable trait's module; every other builtin trait (`TryFrom`, `Error`)
        // names its declaring module itself.
        let segments = stdlib::trait_method_module_segments(name).or_else(|| {
            incan_lang::lang::traits::from_str(name)
                .and_then(incan_lang::lang::traits::source_module)
                .map(|module| module.split('.').map(str::to_string).collect())
        })?;
        let info = self.loaf_trait(&segments, name)?;
        self.standard_traits.insert(name.to_string(), info.clone());
        Some(info)
    }

    /// The path `library`'s public modules are bound under (see [`LoafEntry::binding_root`]).
    pub(in crate::typechecker) fn loaf_binding_root(&self, library: &str) -> Vec<String> {
        self.loaves
            .entry(library)
            .map(LoafEntry::binding_root)
            .unwrap_or_else(|| vec!["pub".to_string(), library.to_string()])
    }

    /// Identify an admitted module by its provider package while its lexical binding keeps the dependency alias.
    /// Standard namespaces retain their `std` module origins; a consumer's package alias never becomes a declaring
    /// package in executable requirements.
    pub(super) fn loaf_module_identity(&self, library: &str, module: &[String]) -> Option<CanonicalSymbolId> {
        let entry = self.loaves.entry(library)?;
        let mut root = entry.binding_root();
        if let [namespace, owner] = root.as_mut_slice()
            && namespace == "pub"
        {
            owner.clone_from(&entry.artifact.name);
        }
        root.extend_from_slice(module);
        SymbolTable::module_path_identity(&root)
    }

    /// The Loaf import a `std.*` module path reaches: the standard package owning its namespace root, and the module
    /// path inside that package's manifest.
    pub(in crate::typechecker) fn standard_loaf_import(&self, segments: &[String]) -> Option<LoafImport> {
        let entry = self.loaves.owner_of(segments)?;
        Some(LoafImport {
            key: entry.key.clone(),
            module: segments[1..].to_vec(),
        })
    }
}
