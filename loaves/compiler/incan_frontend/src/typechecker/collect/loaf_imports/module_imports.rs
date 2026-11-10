//! Loaf module, `from` and root imports, standard surface and testing marker imports, and capability bounds.

use super::*;

impl TypeChecker {
    /// Bind a compiled module while preserving its written source root for reserved-name validation. A `pub::` import
    /// supplies no standard root exemption, even when its dependency happens to provide standard modules.
    pub(in crate::typechecker) fn collect_loaf_module_import(
        &mut self,
        import: &LoafImport,
        alias: Option<&Ident>,
        source_root: Option<&str>,
        span: Span,
    ) {
        let library = import.key.as_str();
        let name = alias
            .cloned()
            .or_else(|| import.module.last().cloned())
            .unwrap_or_else(|| library.to_string());
        self.validate_module_import_binding(&name, source_root, span);
        let Some(manifest) = self.loaf_manifest_or_refuse(library, span) else {
            if self.loaf_is_not_prepared(library) {
                let mut path = self.loaf_binding_root(library);
                path.extend(import.module.iter().cloned());
                self.define_import_symbol(name, path, false, None, span);
            }
            return;
        };
        if !import.module.is_empty() && !Self::manifest_public_module_exists(&manifest, &import.module) {
            self.push_loaf_module_not_found(library, &manifest, &import.module, span);
            return;
        }
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let mut canonical_path = self.loaf_binding_root(library);
        canonical_path.extend(import.module.iter().cloned());
        let target_identity = self.loaf_module_identity(library, &import.module);
        self.define_import_symbol(name, canonical_path, false, target_identity, span);
    }

    /// The manifest of the Loaf indexed as `library`, or `None` after refusing the unknown key.
    ///
    /// Explicit ownership payloads are published with the provider's source spellings. They are remapped through
    /// each callable's checked declaration scope before entering checked facts, exactly as its signature is. A fixed
    /// nominal payload therefore remains exact without depending on a facade alias or accepting compatibility.
    pub(in crate::typechecker::collect) fn loaf_manifest_or_refuse(
        &mut self,
        library: &str,
        span: Span,
    ) -> Option<std::sync::Arc<LibraryManifest>> {
        if self.loaf_is_not_prepared(library) {
            self.errors.push(
                crate::diagnostics::CompileError::new(
                    format!("imports from Loaf `{library}` are not yet checked"),
                    span,
                )
                .with_stable_code("INCAN-T0134")
                .with_hint("Review the Loaf before adding the trust entry offered by the editor"),
            );
            return None;
        }
        if let Some(entry) = self.loaves.entry(library) {
            let manifest = std::sync::Arc::clone(&entry.artifact.manifest);
            // A provider's checked contracts govern calls to its callables exactly as a local declaration's do.
            if manifest.manifest_format == crate::library_manifest::OWNERSHIP_MANIFEST_FORMAT {
                match crate::library_manifest::ownership::exported_contracts(&manifest) {
                    Ok(contracts) => {
                        let root_remapping = self.public_library_canonical_type_remapping(library, &manifest);
                        self.register_pub_library_declarations(library, &manifest, &root_remapping);
                        for (identity, mut contract) in contracts {
                            let mut type_remapping = root_remapping.clone();
                            if let SymbolOrigin::Package { module_path, .. } = &identity.origin {
                                type_remapping.extend(self.pub_library_declaration_scope(
                                    library,
                                    &manifest,
                                    module_path,
                                ));
                            }
                            for mode in &mut contract.modes {
                                Self::remap_resolved_type_with_import_aliases(&mut mode.underlying, &type_remapping);
                            }
                            self.type_info
                                .declarations
                                .ownership_contracts
                                .entry(identity)
                                .or_insert(contract);
                        }
                    }
                    Err(error) => {
                        self.errors.push(
                            crate::diagnostics::CompileError::type_error(
                                format!(
                                    "Provider '{library}' publishes an ownership contract this compiler cannot read"
                                ),
                                span,
                            )
                            .with_stable_code("INCAN-T0123")
                            .with_note(error.to_string()),
                        );
                        return None;
                    }
                }
            }
            return Some(manifest);
        }
        let known = self.loaves.keys().map(str::to_owned).collect::<Vec<_>>();
        self.errors.push(errors::unknown_pub_library(library, &known, span));
        None
    }

    /// Whether unattended preparation explicitly withheld this dependency's compiled interface.
    pub(in crate::typechecker::collect) fn loaf_is_not_prepared(&self, library: &str) -> bool {
        self.not_prepared_loaves.contains(library)
    }

    /// Refuse a module path the Loaf's manifest does not publish, listing the modules it does.
    pub(in crate::typechecker::collect) fn push_loaf_module_not_found(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        module: &[Ident],
        span: Span,
    ) {
        let available = Self::manifest_public_module_paths(manifest)
            .into_iter()
            .map(|module_path| module_path.join("."))
            .collect::<Vec<_>>();
        self.errors
            .push(errors::pub_library_module_not_found(library, module, &available, span));
    }

    /// The names `from <module> import …` may bind from one Loaf module, for refusals.
    pub(in crate::typechecker::collect) fn loaf_module_member_names(
        manifest: &LibraryManifest,
        module: &[Ident],
    ) -> Vec<String> {
        if !module.is_empty() {
            return Self::public_module_member_names(manifest, module);
        }
        let mut names = Self::manifest_export_names(manifest);
        names.extend(
            Self::manifest_public_module_paths(manifest)
                .into_iter()
                .filter_map(|module_path| module_path.first().cloned()),
        );
        names.sort();
        names.dedup();
        names
    }

    /// Bind `from <module> import item, …` from a compiled Loaf. Builtin trait placeholders are replaced through
    /// ordinary import binding so the manifest supplies their parameters, declaration identities, and accepted import
    /// paths together.
    pub(in crate::typechecker) fn collect_loaf_from_imports(
        &mut self,
        import: &LoafImport,
        items: &[ImportItem],
        span: Span,
    ) {
        let library = import.key.as_str();
        let path = import.module.as_slice();
        let Some(manifest) = self.loaf_manifest_or_refuse(library, span) else {
            if self.loaf_is_not_prepared(library) {
                let mut source_path = self.loaf_binding_root(library);
                source_path.extend(path.iter().cloned());
                for item in items {
                    let local_name = Self::import_item_local_name(item);
                    if self.validate_protected_builtin_binding(&local_name, span) {
                        continue;
                    }
                    self.validate_root_namespace(&local_name, span);
                    let mut binding_path = source_path.clone();
                    binding_path.push(item.name.clone());
                    self.symbols.define_import_binding_at_path(
                        Symbol {
                            name: local_name,
                            kind: SymbolKind::Variable(VariableInfo {
                                ty: ResolvedType::Unknown,
                                is_mutable: false,
                                is_used: false,
                            }),
                            span,
                            scope: 0,
                        },
                        None,
                        binding_path,
                    );
                }
            }
            return;
        };
        if !path.is_empty() && !Self::manifest_public_module_exists(&manifest, path) {
            self.push_loaf_module_not_found(library, &manifest, path, span);
            return;
        }
        self.cache_transitive_pub_export_semantics(library, &manifest);
        let available_exports = Self::loaf_module_member_names(&manifest, path);
        let root = self.loaf_binding_root(library);
        let mut source_module = root.clone();
        source_module.extend(path.iter().cloned());
        for item in items {
            let local_name = Self::import_item_local_name(item);
            if self.validate_protected_builtin_binding(&local_name, span) {
                continue;
            }
            self.validate_root_namespace(&local_name, span);
            if !self.loaf_import_needs_binding(&local_name, &source_module, &item.name, span) {
                continue;
            }
            let imported_type_aliases = self.public_library_type_import_remapping(library, &manifest);
            if path.is_empty() {
                self.collect_loaf_root_from_import(import, &manifest, item, local_name, &imported_type_aliases, span);
                continue;
            }
            let mut child_module_path = path.to_vec();
            child_module_path.push(item.name.clone());
            let imports_namespace = Self::manifest_public_module_exists(&manifest, &child_module_path);
            let mut namespace_path = root.clone();
            namespace_path.extend(child_module_path.iter().cloned());

            let resolved = match self.public_module_member(library, &manifest, path, &item.name) {
                Ok(resolved) => resolved,
                Err(source_modules) => {
                    self.errors.push(errors::pub_library_module_member_ambiguous(
                        library,
                        path,
                        &item.name,
                        &source_modules,
                        span,
                    ));
                    continue;
                }
            };
            if let Some(member) = resolved {
                self.define_pub_module_import_symbol(library, local_name.clone(), namespace_path, member, span);
                self.record_standard_surface_import(&root, path, item, &local_name, span);
                continue;
            }
            if imports_namespace {
                let target_identity = self.loaf_module_identity(library, &child_module_path);
                self.define_import_symbol(local_name, namespace_path, false, target_identity, span);
                continue;
            }
            if let Some(migration) = Self::made_private_type(&manifest, path, &item.name) {
                self.errors.push(errors::import_made_private(
                    &item.name,
                    &format!("{}.{}", root.join("."), path.join(".")),
                    migration.facts.message.as_deref(),
                    migration.facts.replacement.as_deref(),
                    span,
                ));
                continue;
            }
            self.errors.push(errors::pub_library_symbol_not_exported(
                &item.name,
                &format!("{}.{}", root.join("."), path.join(".")),
                &available_exports,
                span,
            ));
        }
    }

    /// Bind a package-root member after collision checks. The caller has proved that the import's module path is empty;
    /// namespace/function ambiguity and feature-visibility rules remain shared with ordinary imports.
    pub(super) fn collect_loaf_root_from_import(
        &mut self,
        import: &LoafImport,
        manifest: &LibraryManifest,
        item: &ImportItem,
        local_name: String,
        imported_type_aliases: &HashMap<String, String>,
        span: Span,
    ) {
        let library = import.key.as_str();
        let root = self.loaf_binding_root(library);
        let child_module_path = vec![item.name.clone()];
        let imports_namespace = Self::manifest_public_module_exists(manifest, &child_module_path);
        let mut namespace_path = root.clone();
        namespace_path.extend(child_module_path.iter().cloned());
        let flat_function = self.pub_library_function_symbol(manifest, &item.name);
        let flat_export = Self::find_manifest_export(manifest, &item.name);
        if imports_namespace && (flat_function.is_some() || flat_export.is_some()) {
            self.errors.push(errors::pub_library_module_member_ambiguous(
                library,
                &[],
                &item.name,
                &[
                    format!("package-root export `{}`", item.name),
                    format!("module `{}`", child_module_path.join(".")),
                ],
                span,
            ));
            return;
        }
        if imports_namespace {
            let target_identity = self.loaf_module_identity(library, &child_module_path);
            self.define_import_symbol(local_name, namespace_path, false, target_identity, span);
            return;
        }

        if let Some(mut kind) = flat_function {
            let canonical = manifest
                .contract_metadata
                .identity_graph
                .canonical_for_public_name(&item.name);
            self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
            self.remap_symbol_kind_with_import_aliases(&mut kind, imported_type_aliases);
            let mut binding_path = root;
            binding_path.push(item.name.clone());
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
            if self.symbols.is_active_lookup_binding(symbol_id) {
                if let Some(identity) = canonical {
                    self.type_info
                        .declarations
                        .resolved_import_identities
                        .insert(local_name.clone(), identity);
                }
                self.record_imported_function_binding(&local_name, &kind);
            }
            return;
        }

        let Some(export) = flat_export else {
            if let Some(feature_sets) = Self::inactive_pub_export_features(manifest, &item.name) {
                self.errors.push(errors::pub_library_symbol_requires_features(
                    &item.name,
                    library,
                    &feature_sets,
                    span,
                ));
            } else {
                match Self::made_private_type(manifest, &[], &item.name) {
                    Some(migration) => self.errors.push(errors::import_made_private(
                        &item.name,
                        library,
                        migration.facts.message.as_deref(),
                        migration.facts.replacement.as_deref(),
                        span,
                    )),
                    None => self.errors.push(errors::pub_library_symbol_not_exported(
                        &item.name,
                        library,
                        &Self::loaf_module_member_names(manifest, &[]),
                        span,
                    )),
                }
            }
            return;
        };

        self.define_pub_import_symbol(
            library,
            manifest,
            &item.name,
            local_name,
            export,
            imported_type_aliases,
            span,
        );
    }

    /// Find one provider-published migration refusal for an exact module and type name.
    fn made_private_type<'a>(
        manifest: &'a LibraryManifest,
        module_path: &[String],
        name: &str,
    ) -> Option<&'a crate::library_manifest::MadePrivateTypeExport> {
        manifest
            .made_private_types
            .iter()
            .find(|migration| migration.module_path == module_path && migration.name == name)
    }

    /// Record the compiler-known surface a `std.*` import binds: surface types (`Html`), surface functions and the
    /// `std.testing` markers. Their declarations come from the manifest like any other; this only records which local
    /// name now spells the surface.
    pub(in crate::typechecker::collect) fn record_standard_surface_import(
        &mut self,
        root: &[String],
        module: &[Ident],
        item: &ImportItem,
        local_name: &str,
        span: Span,
    ) {
        if root != [stdlib::STDLIB_ROOT] {
            return;
        }
        let Some(symbol_id) = self.symbols.lookup(local_name) else {
            return;
        };
        let dotted = std::iter::once(stdlib::STDLIB_ROOT)
            .chain(module.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(".");
        if let Some(surface_type) = surface_types::from_str(&item.name)
            && surface_types::is_exported_from_module(surface_type, &dotted)
        {
            self.surface_type_import_bindings
                .insert(local_name.to_string(), (surface_type, symbol_id));
        }
        if let Some(surface_function) = surface_functions::from_str(&item.name) {
            self.surface_function_import_bindings
                .insert(local_name.to_string(), (surface_function, symbol_id));
        }
        if dotted == "std.testing" {
            self.record_testing_marker_import(module, item, local_name, span);
        }
    }

    /// Record an imported `std.testing` marker alias so decorator validation can reject runtime calls consistently.
    ///
    /// The markers are read from the checked metadata of the standard package that owns `std.testing`.
    pub(in crate::typechecker::collect) fn record_testing_marker_import(
        &mut self,
        module: &[Ident],
        item: &ImportItem,
        local_name: &str,
        span: Span,
    ) {
        let mut module_path = vec![stdlib::STDLIB_ROOT.to_string()];
        module_path.extend(module.iter().cloned());
        let Some(manifest) = self.loaves.owner_of(&module_path).map(|entry| &entry.artifact.manifest) else {
            return;
        };
        let semantics = match testing_marker_semantics_from_manifest(manifest) {
            Ok(Some(semantics)) => semantics,
            Ok(None) => return,
            Err(error) => {
                self.errors
                    .push(errors::invalid_std_testing_marker_metadata(&error.to_string(), span));
                return;
            }
        };
        let mut marker_path = module_path;
        marker_path.push(item.name.clone());
        let module_feature = self.surface_context.decorator_feature_for_path(&marker_path);
        let is_marker = semantics.marker_kind(&item.name).is_some();
        if module_feature == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction)) && is_marker
        {
            self.testing_marker_import_bindings.insert(local_name.to_string());
        }
        self.testing_marker_semantics = Some(semantics);
    }

    /// Bind `from std.rust import <bound>`: the typechecker-only capability bounds, which no Loaf declares.
    pub(in crate::typechecker) fn collect_capability_bound_import(
        &mut self,
        module: &ImportPath,
        item: &ImportItem,
        span: Span,
    ) {
        if !is_rust_capability_bound(item.name.as_str()) {
            self.errors.push(errors::stdlib_import_not_exported(
                &item.name,
                &module.segments.join("."),
                span,
            ));
            return;
        }
        self.define_from_import_symbol(
            module,
            item,
            SymbolKind::Trait(TraitInfo {
                type_params: vec![],
                methods: HashMap::new(),
                method_aliases: HashMap::new(),
                properties: HashMap::new(),
                requires: vec![],
                supertraits: vec![],
            }),
            span,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An import withheld by unattended preparation reports one stable fact instead of an unknown-package error.
    #[test]
    fn not_prepared_loaf_import_has_no_follow_on_error() -> Result<(), Box<dyn std::error::Error>> {
        let source = "from pub::words import marker\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_not_prepared_loaves(HashSet::from(["words".to_string()]));

        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("expected not-prepared diagnostic")?;

        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].stable_code(), Some("INCAN-T0134"));
        Ok(())
    }
}
