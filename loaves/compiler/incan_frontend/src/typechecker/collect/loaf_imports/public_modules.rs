//! Public module paths, members and namespace matches, and public module import symbols.

use super::*;

impl TypeChecker {
    /// Return the public source spelling for one checked API declaration.
    pub(in crate::typechecker::collect) fn api_declaration_name(declaration: &ApiDeclaration) -> &str {
        match declaration {
            ApiDeclaration::Function(item) => &item.name,
            ApiDeclaration::Model(item) => &item.name,
            ApiDeclaration::Class(item) => &item.name,
            ApiDeclaration::Trait(item) => &item.name,
            ApiDeclaration::Enum(item) => &item.name,
            ApiDeclaration::Newtype(item) => &item.name,
            ApiDeclaration::TypeAlias(item) => &item.name,
            ApiDeclaration::Const(item) => &item.name,
            ApiDeclaration::Static(item) => &item.name,
            ApiDeclaration::Alias(item) => &item.name,
            ApiDeclaration::Partial(item) => &item.name,
        }
    }

    /// Return all derived public module namespace paths carried by the checked API artifact.
    pub(in crate::typechecker::collect) fn manifest_public_module_paths(
        manifest: &LibraryManifest,
    ) -> Vec<Vec<String>> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        checked_api_public_module_paths(api)
    }

    /// Return whether the checked API artifact contains this public module namespace.
    pub(in crate::typechecker) fn manifest_public_module_exists(
        manifest: &LibraryManifest,
        module_path: &[String],
    ) -> bool {
        !module_path.is_empty()
            && Self::manifest_public_module_paths(manifest)
                .iter()
                .any(|candidate| candidate == module_path)
    }

    /// Return source API modules whose public declarations participate directly in one derived namespace.
    ///
    /// A directory namespace exposes declarations from its own entrypoint and its immediate source-file children.
    /// Deeper directories remain nested namespaces rather than leaking all descendants into every ancestor.
    pub(in crate::typechecker::collect) fn api_modules_for_public_namespace<'a>(
        manifest: &'a LibraryManifest,
        module_path: &[String],
    ) -> Vec<&'a crate::api_metadata::CheckedApiMetadata> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        checked_api_modules_for_public_namespace(api, module_path)
    }

    /// Return the public names available directly from one derived module namespace.
    pub(in crate::typechecker::collect) fn public_module_member_names(
        manifest: &LibraryManifest,
        module_path: &[String],
    ) -> Vec<String> {
        let Some(api) = manifest.contract_metadata.api.as_ref() else {
            return Vec::new();
        };
        let Some(namespace) = checked_api_public_namespace(api, module_path) else {
            return Vec::new();
        };
        let mut names = namespace
            .members
            .into_iter()
            .map(|member| member.name)
            .chain(namespace.child_modules)
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    /// Resolve one declaration from a derived public module, preserving its authored source-module identity.
    ///
    /// External aliases carry admitted callable or nominal projections; consume those before following a target
    /// inside this manifest, since another provider's declaration is not present in the facade's source modules.
    pub(in crate::typechecker::collect) fn public_module_member(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        module_path: &[String],
        member: &str,
    ) -> Result<Option<PublicModuleMember>, Vec<String>> {
        let matches = Self::api_modules_for_public_namespace(manifest, module_path)
            .into_iter()
            .filter_map(|module| {
                let declarations = module
                    .declarations
                    .iter()
                    .filter(|declaration| {
                        checked_api_declaration_is_public_namespace_member(declaration)
                            && Self::api_declaration_name(declaration) == member
                    })
                    .collect::<Vec<_>>();
                (!declarations.is_empty()).then_some((module.module_path.clone(), declarations))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Ok(None);
        }

        let source_modules = matches
            .iter()
            .map(|(source_module, _)| source_module.join("."))
            .collect::<Vec<_>>();
        let (source_module_path, declarations) = match matches.as_slice() {
            [only] => only,
            _ => Self::namespace_own_module_match(module_path, &matches)
                .or_else(|| Self::one_declaration_behind_namespace_matches(manifest, member, &matches))
                .ok_or_else(|| source_modules.clone())?,
        };
        // Reconstruct foreign projected layouts before reading a projected declaration. The raw declaring artifact
        // spells sibling return types in its own package scope; the retained layout carries the provider-qualified
        // spellings a consumer can resolve without importing those siblings by name.
        let remapping = self.public_module_type_remapping(library, manifest, source_module_path);
        let mut kinds = declarations
            .iter()
            .filter_map(|declaration| match declaration {
                ApiDeclaration::Alias(alias) => self
                    .symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration)
                    .or_else(|| {
                        self.symbol_kind_from_api_target_path(
                            manifest,
                            Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
                        )
                    }),
                _ => self.symbol_kind_from_api_declaration(manifest.contract_metadata.api.as_ref(), declaration),
            })
            .collect::<Vec<_>>();
        let mut type_alias = declarations
            .iter()
            .find_map(|declaration| Self::public_member_type_alias(manifest, source_module_path, declaration));
        let (resolved_source_module_path, resolved_source_name) =
            declarations
                .iter()
                .find_map(|declaration| match declaration {
                    ApiDeclaration::Alias(alias) => Self::normalized_api_declaration_target(
                        Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
                    ),
                    _ => None,
                })
                .unwrap_or_else(|| (source_module_path.clone(), member.to_string()));
        let public_path = std::iter::once(manifest.name.clone())
            .chain(module_path.iter().cloned())
            .chain(std::iter::once(member.to_string()))
            .collect::<Vec<_>>();
        let source_path = resolved_source_module_path
            .iter()
            .cloned()
            .chain(std::iter::once(resolved_source_name.clone()))
            .collect::<Vec<_>>();
        let overload_identities = manifest
            .contract_metadata
            .identity_graph
            .identities_for_public_source_path(&public_path, &source_path);
        let mut kind = match kinds.as_mut_slice() {
            [kind] => kind.clone(),
            kinds if !kinds.is_empty() && kinds.iter().all(|kind| matches!(kind, SymbolKind::Function(_))) => {
                SymbolKind::FunctionOverloads(
                    kinds
                        .iter()
                        .enumerate()
                        .filter_map(|(index, kind)| match kind {
                            SymbolKind::Function(info) => Some(FunctionOverloadInfo {
                                info: info.clone(),
                                span: Span::default(),
                                identity: overload_identities.get(index).cloned().flatten(),
                            }),
                            _ => None,
                        })
                        .collect(),
                )
            }
            _ => return Err(source_modules),
        };
        let canonical = overload_identities.first().cloned().flatten().filter(|first| {
            overload_identities
                .iter()
                .all(|identity| identity.as_ref() == Some(first))
        });
        self.remap_symbol_kind_through_declaring_module(library, manifest, canonical.as_ref(), &mut kind);
        self.remap_symbol_kind_with_import_aliases(&mut kind, &remapping);
        if let Some((_, target)) = &mut type_alias {
            Self::remap_resolved_type_with_import_aliases(target, &remapping);
        }
        let partial_projection = declarations.iter().find_map(|declaration| {
            Self::public_member_partial_projection(manifest, source_module_path, member, declaration, &remapping)
        });
        Self::mark_compiled_class_field_provider(&mut kind, library);
        Ok(Some(PublicModuleMember {
            kind,
            canonical,
            source_module_path: resolved_source_module_path,
            source_name: resolved_source_name,
            type_alias,
            partial_projection,
        }))
    }

    /// Read a public alias's type parameters and target through the retained declaration. Native union projections
    /// already carry their closed target and need no second declaration lookup.
    pub(super) fn public_member_type_alias(
        manifest: &LibraryManifest,
        source_module_path: &[String],
        declaration: &ApiDeclaration,
    ) -> Option<(Vec<TypeParamExport>, ResolvedType)> {
        if let ApiDeclaration::Alias(alias) = declaration
            && let Some(target) = alias.projected_type.as_ref().filter(|ty| ty.has_native_union())
        {
            return Some((Vec::new(), resolved_type_from_manifest_type_ref(target)));
        }
        let declaration = match declaration {
            ApiDeclaration::Alias(alias) => Self::api_declaration_for_target_path(
                manifest,
                Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
            )?,
            declaration => declaration,
        };
        match declaration {
            ApiDeclaration::TypeAlias(item) => Some((
                item.type_alias.type_params.clone(),
                resolved_type_from_manifest_type_ref(&item.type_alias.target),
            )),
            _ => None,
        }
    }

    /// Preserve partial application metadata when a public namespace reaches it through a declaration alias.
    pub(super) fn public_member_partial_projection(
        manifest: &LibraryManifest,
        source_module_path: &[String],
        member: &str,
        declaration: &ApiDeclaration,
        remapping: &HashMap<String, String>,
    ) -> Option<PartialProjectionInfo> {
        let declaration = match declaration {
            ApiDeclaration::Alias(alias) => Self::api_declaration_for_target_path(
                manifest,
                Self::api_alias_target_path(manifest, source_module_path, &alias.name, &alias.target_path),
            )?,
            declaration => declaration,
        };
        let ApiDeclaration::Partial(partial) = declaration else {
            return None;
        };
        let export = partial_export_from_api(partial);
        Self::partial_projection_from_manifest_partial(&export, member, remapping, Span::default())
    }

    /// Choose one of several source modules that each publish `member` into the same derived namespace, when the
    /// identity graph says they all publish the same declarations.
    ///
    /// Select the declaration the namespace's own module makes when an immediate child module declares the same name.
    ///
    /// A module such as `snappy` that owns the default surface of its namespace and keeps an alternative (`snappy.raw`)
    /// in a child module is not ambiguous: the own module's declaration takes precedence, as a declared member does
    /// over a child namespace, and the child stays reachable through its exact module path. Two child modules that
    /// declare the name remain ambiguous.
    pub(in crate::typechecker::collect) fn namespace_own_module_match<'m, 'a>(
        namespace_path: &[String],
        matches: &'m [(Vec<String>, Vec<&'a ApiDeclaration>)],
    ) -> Option<&'m (Vec<String>, Vec<&'a ApiDeclaration>)> {
        matches
            .iter()
            .find(|(module_path, _)| module_path.as_slice() == namespace_path)
    }

    /// A directory namespace exposes its entrypoint's declarations and its source-file children's, so a facade that
    /// re-exports a child's declaration (`pkg/mod.incn` holding `pub from crate.pkg.inner import Greeting`) puts the
    /// name there twice. Each source module publishes the member at its own namespace path, and the identity graph
    /// records the canonical declarations there; when every module's set is the same, the spellings name one
    /// declaration and nothing is ambiguous. The declaring module's match is preferred, so the member keeps its
    /// declaration rather than a forwarding alias. Different or unproven identities stay ambiguous.
    pub(in crate::typechecker::collect) fn one_declaration_behind_namespace_matches<'m, 'a>(
        manifest: &LibraryManifest,
        member: &str,
        matches: &'m [(Vec<String>, Vec<&'a ApiDeclaration>)],
    ) -> Option<&'m (Vec<String>, Vec<&'a ApiDeclaration>)> {
        let graph = &manifest.contract_metadata.identity_graph;
        let canonicals_of = |module_path: &[String]| {
            let public_path = std::iter::once(manifest.name.clone())
                .chain(module_path.iter().cloned())
                .chain(std::iter::once(member.to_string()))
                .collect::<Vec<_>>();
            let mut canonicals = graph.canonicals_at_public_path(&public_path);
            canonicals.sort();
            canonicals
        };
        let (first, rest) = matches.split_first()?;
        let published = canonicals_of(&first.0);
        if published.is_empty()
            || rest
                .iter()
                .any(|(module_path, _)| canonicals_of(module_path) != published)
        {
            return None;
        }
        matches
            .iter()
            .find(|(_, declarations)| {
                declarations
                    .iter()
                    .any(|declaration| !matches!(declaration, ApiDeclaration::Alias(_)))
            })
            .or(Some(first))
    }

    /// Build canonical and local type spellings for API declarations consumed through public module namespaces.
    pub(in crate::typechecker::collect) fn public_module_type_remapping(
        &mut self,
        library: &str,
        manifest: &LibraryManifest,
        owner_module_path: &[String],
    ) -> HashMap<String, String> {
        let mut remapping = self.public_library_canonical_type_remapping(library, manifest);
        self.register_pub_library_declarations(library, manifest, &remapping);
        remapping.extend(self.pub_library_declaration_scope(library, manifest, owner_module_path));
        remapping
    }

    /// Define one declaration imported through a derived public module namespace, retaining provider identity for
    /// both nominal types and trait types so an imported alias matches its declaring-module signature spelling.
    pub(in crate::typechecker::collect) fn define_pub_module_import_symbol(
        &mut self,
        library: &str,
        local_name: String,
        binding_path: Vec<String>,
        member: PublicModuleMember,
        span: Span,
    ) {
        let type_alias_target = member
            .type_alias
            .map(|(type_params, target)| crate::typechecker::TypeAliasTarget {
                type_params: type_params.iter().map(|param| param.name.clone()).collect(),
                target,
                nominal_identities: Default::default(),
            });
        let mut partial_projection = member.partial_projection;
        let canonical = member.canonical;
        let source_module_path = member.source_module_path;
        let source_name = member.source_name;
        let mut kind = member.kind;
        Self::mark_compiled_class_field_provider(&mut kind, library);
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

        if let Some(target) = type_alias_target {
            self.record_dependency_import_type_alias_before_change(&local_name);
            self.type_aliases.insert(local_name.clone(), target);
        }
        self.record_imported_function_binding(&local_name, &kind);
        if let Some(projection) = &mut partial_projection {
            projection.name.clone_from(&local_name);
        }
        if let Some(projection) = partial_projection {
            self.type_info.record_partial_projection(projection);
        }
        if let Some(target_kind) = Self::source_target_kind(&kind) {
            let mut target_path = self.loaf_binding_root(library);
            target_path.extend(source_module_path.clone());
            self.source_import_targets.insert(
                local_name.clone(),
                crate::typechecker::SourceTargetInfo {
                    module_path: target_path,
                    name: source_name.clone(),
                    kind: target_kind.to_string(),
                },
            );
        }
        if matches!(
            kind,
            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Enum(_) | TypeInfo::Newtype(_))
                | SymbolKind::Trait(_)
        ) {
            let mut source_path = source_module_path;
            source_path.push(source_name);
            let declaration_identity = self
                .type_info
                .declarations
                .resolved_import_identities
                .get(&local_name)
                .cloned();
            let identity = self.admitted_public_type_identity(library, &source_path, declaration_identity);
            self.public_library_type_identities.insert(local_name, identity);
        }
    }
}
