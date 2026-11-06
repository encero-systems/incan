//! Dependency registry definitions, adopted traits, partial projections, and derivable metadata.

use super::*;

impl TypeChecker {
    /// Cache exact provider traits referenced by dependency-owned type adoptions.
    ///
    /// A dependency may write `@derive(json)` and therefore store `json.Serialize` on its exported model. The
    /// dependency's local `json` binding must not become ambient in a consumer, but method and bound resolution still
    /// need the trait interface. Cache it under the provider module plus source trait name; lookup remains
    /// semantic-only through [`Self::lookup_trait_adoption_info`].
    pub(super) fn cache_dependency_adoption_traits(&mut self) {
        let adoptions = self
            .dependency_direct_member_symbols
            .values()
            .flat_map(HashMap::values)
            .filter_map(|kind| match kind {
                SymbolKind::Type(TypeInfo::Model(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Class(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Newtype(info)) => Some(info.trait_adoptions.as_slice()),
                SymbolKind::Type(TypeInfo::Enum(info)) => Some(info.trait_adoptions.as_slice()),
                _ => None,
            })
            .flatten()
            .filter_map(|adoption| {
                let module_path = adoption.module_path.as_ref()?;
                let source_name = adoption
                    .source_name
                    .as_deref()
                    .or_else(|| adoption.name.rsplit('.').next())?;
                Some((module_path.clone(), source_name.to_string()))
            })
            .collect::<Vec<_>>();

        for (module_path, source_name) in adoptions {
            let key = format!("{}.{}", module_path.join("."), source_name);
            if self.dependency_module_traits.contains_key(&key) {
                continue;
            }
            if let Some((library, rest)) = self.loaf_module(&module_path)
                && let Some((SymbolKind::Trait(info), _)) =
                    self.lookup_pub_library_module_symbol_member(&library, &rest, &source_name)
            {
                self.dependency_module_traits.insert(key, info);
            }
        }
    }

    /// Cache canonical `Registry.define(...)` contracts from one dependency's owned static declarations.
    ///
    /// This accepts only actual `Registry.define(subjects=[...])` initializers. An imported `Registry[K, T]` type
    /// therefore never acquires registry authority by itself, while a consumer can use the defining contract without
    /// creating a second registry definition in its own `TypeCheckInfo`.
    pub(super) fn cache_dependency_registry_definitions(
        &mut self,
        module_name: &str,
        module_ast: &Program,
        public_only: bool,
    ) {
        let mut definitions = HashMap::new();
        for declaration in &module_ast.declarations {
            let Declaration::Static(static_decl) = &declaration.node else {
                continue;
            };
            if public_only && static_decl.visibility != Visibility::Public {
                continue;
            }
            let static_type = self.resolve_type_checked(&static_decl.ty);
            let Some((key_type, descriptor_type)) = Self::registry_type_arguments(&static_type) else {
                continue;
            };
            let Ok(subjects) = self.registry_definition_subjects(static_decl) else {
                continue;
            };
            definitions.insert(
                static_decl.name.clone(),
                RegistryDefinitionInfo {
                    key_type: key_type.clone(),
                    descriptor_type: descriptor_type.clone(),
                    subjects,
                    is_public: static_decl.visibility == Visibility::Public,
                },
            );
        }
        self.dependency_registry_definitions.insert(
            module_name.to_string(),
            DependencyRegistryDefinitions {
                owner_module_path: Self::dependency_registry_owner_module_path(module_ast, module_name),
                definitions,
            },
        );
    }

    /// Record each `pub const` of one dependency module as the registry structural value it denotes, computed with the
    /// module's own consts and names, so a consumer's descriptor can name it as it names a local const (#2605). A
    /// const that is not structural, or whose expansion is cyclic, records its refusal instead.
    pub(super) fn cache_dependency_registry_consts(&mut self, module_ast: &Program) {
        let module_consts = module_ast
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                Declaration::Const(konst) => Some((konst.name.clone(), (konst.clone(), declaration.span))),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        let consumer_consts = std::mem::replace(&mut self.const_decls, module_consts);
        let mut values = Vec::new();
        for declaration in &module_ast.declarations {
            let Declaration::Const(konst) = &declaration.node else {
                continue;
            };
            if konst.visibility != Visibility::Public {
                continue;
            }
            let Some(identity) = self
                .symbols
                .lookup(&konst.name)
                .and_then(|id| self.symbols.identity_of(id))
                .filter(|identity| identity.kind == SemanticSourceTargetKind::Const)
                .cloned()
            else {
                continue;
            };
            values.push((identity, self.registry_structural_value(&konst.value)));
        }
        self.const_decls = consumer_consts;
        self.dependency_registry_consts.extend(values);
    }

    /// Recover the source-module identity for a dependency registry without inferring it from its lookup key.
    ///
    /// The parser records the module file path for normal compilation. Its path below `src/` is the source authority
    /// for metadata identity. Inline unit tests do not carry a file path, so their explicitly supplied dependency
    /// name remains the narrow test-only fallback.
    pub(super) fn dependency_registry_owner_module_path(module_ast: &Program, module_name: &str) -> Vec<String> {
        let Some(source_path) = module_ast.source_path.as_deref() else {
            return vec![module_name.to_string()];
        };
        let path = std::path::Path::new(source_path);
        let mut after_src = false;
        let mut segments = Vec::new();
        for component in path.components() {
            let std::path::Component::Normal(component) = component else {
                continue;
            };
            let Some(component) = component.to_str() else {
                return vec![module_name.to_string()];
            };
            if component == "src" {
                after_src = true;
                segments.clear();
            } else if after_src {
                segments.push(component.to_string());
            }
        }
        let Some(last) = segments.last_mut() else {
            return vec![module_name.to_string()];
        };
        let Some(stem) = std::path::Path::new(last).file_stem().and_then(|stem| stem.to_str()) else {
            return vec![module_name.to_string()];
        };
        *last = stem.to_string();
        if matches!(last.as_str(), "mod" | "__init__") {
            segments.pop();
        }
        if segments.is_empty() {
            vec![module_name.to_string()]
        } else {
            canonicalize_source_module_segments(&segments)
        }
    }

    /// Return one imported registry's checked defining contract and canonical owner path.
    ///
    /// Source import resolution uses the same candidate order as member-symbol resolution. The returned owner path is
    /// the resolved dependency path rather than the consumer's local alias, preserving one registry identity through
    /// typechecking, lowering, and package metadata.
    pub fn dependency_registry_definition_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<(RegistryDefinitionInfo, Vec<String>)> {
        let current_module_path = self.current_module_path.as_deref().unwrap_or_default();
        for candidate in self.dependency_source_import_candidates(current_module_path, module) {
            let key = canonicalize_source_module_segments(&candidate).join("_");
            if let Some(registries) = self.dependency_registry_definitions.get(&key)
                && let Some(definition) = registries.definitions.get(item_name)
            {
                return Some((definition.clone(), registries.owner_module_path.clone()));
            }
        }
        if module.is_absolute || module.parent_levels > 0 {
            return None;
        }

        let suffix = canonicalize_source_module_segments(&module.segments).join("_");
        if suffix.is_empty() {
            return None;
        }
        let suffix = format!("_{suffix}");
        let mut matches = self
            .dependency_registry_definitions
            .iter()
            .filter_map(|(module_key, definitions)| {
                module_key
                    .ends_with(&suffix)
                    .then(|| {
                        definitions
                            .definitions
                            .get(item_name)
                            .map(|definition| (module_key, definitions, definition))
                    })
                    .flatten()
            });
        let (_module_key, registries, definition) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some((definition.clone(), registries.owner_module_path.clone()))
    }

    /// Return the exact partial projection metadata for one dependency module member path.
    pub fn dependency_member_partial_projection_for_path(
        &self,
        module: &ImportPath,
        item_name: &str,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_module_entry_for_path(module, &self.dependency_member_partial_projections)?
            .get(item_name)
            .cloned()
    }

    /// Return one dependency member's partial projection, resolving against an explicit base module.
    pub fn dependency_member_partial_projection_for_path_from(
        &self,
        base_module_path: &[String],
        module: &ImportPath,
        item_name: &str,
    ) -> Option<PartialProjectionInfo> {
        self.dependency_module_entry_for_path_from(
            base_module_path,
            module,
            &self.dependency_member_partial_projections,
        )?
        .get(item_name)
        .cloned()
    }

    /// Register RFC 024 metadata exported by a dependency source module.
    ///
    /// `__derives__` is compiler metadata rather than a public const, so consumers need this side channel even when the
    /// declaration itself is not imported as a symbol.
    pub(super) fn register_dependency_derivable_metadata(&mut self, module_name: &str, module_ast: &Program) {
        let mut module_paths = vec![module_name.to_string()];
        if let Some(segments) = self.canonical_owner_segments(module_name) {
            let dotted = segments.join(".");
            if !module_paths.contains(&dotted) {
                module_paths.push(dotted);
            }
            let public = std::iter::once("std".to_owned()).chain(segments).collect::<Vec<_>>();
            if self.owns_standard_module(&public) {
                module_paths.push(public.join("."));
            }
        }
        let traits = Self::derivable_traits_from_program(module_ast);
        if !traits.is_empty() {
            for module_path in &module_paths {
                self.dependency_derivable_modules
                    .insert(module_path.clone(), traits.clone());
            }
        }

        for decl in &module_ast.declarations {
            let Declaration::Trait(tr) = &decl.node else {
                continue;
            };
            if let Some(info) = self.lookup_trait_info(&tr.name).cloned() {
                for module_path in &module_paths {
                    self.dependency_module_traits
                        .insert(format!("{module_path}.{}", tr.name), info.clone());
                }
            }
            let paths = Self::rust_derive_paths_from_trait(tr);
            if paths.is_empty() {
                continue;
            }
            for module_path in &module_paths {
                let qualified = format!("{module_path}.{}", tr.name);
                self.dependency_trait_rust_derive_paths.insert(qualified, paths.clone());
            }
        }
        self.retain_derivable_trait_bindings(&module_paths, &traits);
    }

    /// Retain each module derive entry while its lexical imports still identify the selected trait declaration.
    /// Imported names are metadata references, not declarations owned by the bundle. Keeping their checked identities,
    /// contracts and native derive paths here prevents a later consumer's equal-spelled binding from answering them.
    pub(super) fn retain_derivable_trait_bindings(&mut self, module_paths: &[String], trait_names: &[String]) {
        for name in trait_names {
            let Some(symbol_id) = self.symbols.lookup(name) else {
                continue;
            };
            let Some(identity) = self.symbols.identity_of(symbol_id).cloned() else {
                continue;
            };
            let Some(SymbolKind::Trait(info)) = self.symbols.get(symbol_id).map(|symbol| &symbol.kind) else {
                continue;
            };
            let info = info.clone();
            let Some((owner, source_name)) = self.canonical_trait_source_target(&identity) else {
                continue;
            };
            let owner_key = format!("{}.{}", owner.join("."), source_name);
            let paths = self
                .dependency_trait_rust_derive_paths
                .get(&owner_key)
                .cloned()
                .or_else(|| self.loaf_trait_rust_derive_paths(&owner, &source_name))
                .unwrap_or_default();
            for module_path in module_paths {
                let key = format!("{module_path}.{name}");
                self.dependency_derivable_trait_identities
                    .insert(key.clone(), identity.clone());
                self.dependency_module_traits.insert(key.clone(), info.clone());
                if !paths.is_empty() {
                    self.dependency_trait_rust_derive_paths.insert(key, paths.clone());
                }
            }
        }
    }

    /// Extract RFC 024 `__derives__` trait names from an imported dependency AST.
    pub fn derivable_traits_from_program(program: &Program) -> Vec<String> {
        for decl in &program.declarations {
            let Declaration::Const(konst) = &decl.node else {
                continue;
            };
            if konst.name != "__derives__" {
                continue;
            }
            let Expr::List(entries) = &konst.value.node else {
                return Vec::new();
            };
            return entries
                .iter()
                .filter_map(|entry| match entry {
                    ListEntry::Element(expr) => match &expr.node {
                        Expr::Ident(name) => Some(name.clone()),
                        _ => None,
                    },
                    ListEntry::Spread(_) => None,
                })
                .collect();
        }
        Vec::new()
    }

    /// Extract `@rust.derive(...)` path strings from one imported trait declaration.
    pub(super) fn rust_derive_paths_from_trait(tr: &TraitDecl) -> Vec<String> {
        let mut paths = Vec::new();
        for decorator in &tr.decorators {
            let path = decorator.node.path.segments.join(".");
            if core_decorators::from_str(&path) != Some(DecoratorId::RustDerive) {
                continue;
            }
            for arg in &decorator.node.args {
                let DecoratorArg::Positional(expr) = arg else {
                    continue;
                };
                let Expr::Literal(Literal::String(path)) = &expr.node else {
                    continue;
                };
                if !paths.iter().any(|existing| existing == path) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }
}
