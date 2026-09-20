//! Import declaration lowering.

use super::super::super::decl::{IrDeclKind, IrRustTraitImport};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_lang::lang::stdlib;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

impl AstLowering {
    /// Lower an import declaration.
    ///
    /// Returns `None` for an item import whose every item is derive vocabulary (see
    /// [`Self::is_derive_vocabulary_trait_import`]): such a declaration binds nothing in generated Rust, and lowering
    /// it as an item-less import would re-export the stdlib module instead.
    pub(in crate::lower) fn lower_import(
        &self,
        i: &ast::ImportDecl,
        span: ast::Span,
    ) -> Result<Option<IrDeclKind>, LoweringError> {
        // The frontend resolves `import module::item` before lowering. Once it proves an item identity, use the
        // same item import route as `from module import item`, including canonical names and alias projections.
        // A genuine module identity must retain module binding semantics even when its path has multiple segments.
        if let ast::ImportKind::Module(path) = &i.kind
            && let Some((name, parent)) = path.segments.split_last()
            && let Some(identity) = self
                .type_info
                .as_ref()
                .and_then(|info| info.resolved_import_identity(i.alias.as_deref().unwrap_or(name)))
            && !matches!(identity.kind, SemanticSourceTargetKind::Module)
            && matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
        {
            let mut item_import = i.clone();
            item_import.kind = ast::ImportKind::From {
                module: ast::ImportPath {
                    segments: parent.to_vec(),
                    is_absolute: path.is_absolute,
                    parent_levels: path.parent_levels,
                },
                items: vec![ast::ImportItem {
                    name: name.clone(),
                    alias: i.alias.clone(),
                }],
            };
            item_import.alias = None;
            return self.lower_import(&item_import, span);
        }

        let (path, ast_items) = match &i.kind {
            ast::ImportKind::Module(p) => (canonicalize_source_module_segments(&p.segments), vec![]),
            ast::ImportKind::From { module, items } => {
                (canonicalize_source_module_segments(&module.segments), items.clone())
            }
            ast::ImportKind::PubLibrary { library, path } => {
                let mut segments = vec![library.clone()];
                segments.extend(path.iter().cloned());
                (segments, vec![])
            }
            ast::ImportKind::PubFrom { library, path, items } => {
                let mut segments = vec![library.clone()];
                segments.extend(path.iter().cloned());
                (segments, items.clone())
            }
            ast::ImportKind::RustCrate { crate_name, path, .. } => {
                let mut segs = vec![crate_name.clone()];
                segs.extend(path.clone());
                (segs, vec![])
            }
            ast::ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..  // Ignore version and features
            } => {
                let mut segs = vec![crate_name.clone()];
                segs.extend(path.clone());
                (segs, items.clone())
            }
            ast::ImportKind::Python(s) => (vec![s.clone()], vec![]),
        };
        let origin = match &i.kind {
            ast::ImportKind::PubLibrary { library, .. } | ast::ImportKind::PubFrom { library, .. } => {
                super::super::super::decl::IrImportOrigin::PubLibrary {
                    dependency_key: library.clone(),
                }
            }
            _ => super::super::super::decl::IrImportOrigin::Standard,
        };

        let qualifier = match &i.kind {
            ast::ImportKind::Module(p) => {
                if p.parent_levels > 0 {
                    super::super::super::decl::IrImportQualifier::Super(p.parent_levels)
                } else if p.is_absolute {
                    super::super::super::decl::IrImportQualifier::Crate
                } else {
                    super::super::super::decl::IrImportQualifier::Auto
                }
            }
            ast::ImportKind::From { module, .. } => {
                if module.parent_levels > 0 {
                    super::super::super::decl::IrImportQualifier::Super(module.parent_levels)
                } else if module.is_absolute {
                    super::super::super::decl::IrImportQualifier::Crate
                } else {
                    super::super::super::decl::IrImportQualifier::Auto
                }
            }
            _ => super::super::super::decl::IrImportQualifier::None,
        };

        // Convert AST import items to IR import items
        let ir_items: Vec<super::super::super::decl::IrImportItem> = ast_items
            .iter()
            .filter(|item| !self.is_derive_vocabulary_trait_import(item.alias.as_ref().unwrap_or(&item.name)))
            .flat_map(|item| {
                let binding_name = item.alias.as_ref().unwrap_or(&item.name);
                let force_reexport = self.overload_alias_reexport_targets.contains(binding_name);
                if let Some(emitted_names) = self.emitted_overload_import_names(binding_name) {
                    return emitted_names
                        .iter()
                        .map(|emitted_name| super::super::super::decl::IrImportItem {
                            name: emitted_name.clone(),
                            alias: None,
                            canonical: self.overload_import_identity(binding_name, emitted_name),
                            is_static: false,
                            force_reexport,
                            rust_trait_import: None,
                        })
                        .collect::<Vec<_>>();
                }
                let rust_trait_import = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.rust.trait_imports.get(binding_name))
                    .map(|import| {
                        let mut methods: Vec<_> = import.methods.iter().cloned().collect();
                        methods.sort();
                        IrRustTraitImport {
                            trait_path: import.trait_path.clone(),
                            definition_path: import.definition_path.clone(),
                            methods,
                            methods_known: import.methods_known,
                        }
                    });
                vec![super::super::super::decl::IrImportItem {
                    name: item.name.clone(),
                    alias: item.alias.clone(),
                    canonical: self
                        .type_info
                        .as_ref()
                        .and_then(|info| info.resolved_import_identity(binding_name))
                        .cloned(),
                    is_static: self
                        .type_info
                        .as_ref()
                        .is_some_and(|info| info.static_binding(binding_name).is_some()),
                    force_reexport: false,
                    rust_trait_import,
                }]
            })
            .collect();
        let mut deduped_items: Vec<super::super::super::decl::IrImportItem> = Vec::new();
        for item in ir_items {
            if let Some(existing) = deduped_items.iter_mut().find(|existing| {
                existing.name == item.name && existing.alias == item.alias && existing.is_static == item.is_static
            }) {
                existing.force_reexport |= item.force_reexport;
                if existing.rust_trait_import.is_none() {
                    existing.rust_trait_import = item.rust_trait_import;
                }
            } else {
                deduped_items.push(item);
            }
        }
        let ir_items = deduped_items;

        if let Some(item) = ir_items.iter().find(|item| {
            item.is_static
                && !item.canonical.as_ref().is_some_and(|identity| {
                    matches!(identity.kind, SemanticSourceTargetKind::Static)
                        && matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
                })
        }) {
            return Err(LoweringError {
                message: format!(
                    "linker-visible Incan static import `{}` reached lowering without its compiler-owned canonical identity",
                    item.source_binding_name()
                ),
                span: span.into(),
            });
        }

        if ir_items.is_empty() && !ast_items.is_empty() {
            return Ok(None);
        }

        Ok(Some(IrDeclKind::Import {
            visibility: Self::map_visibility(i.visibility),
            origin,
            qualifier,
            path,
            alias: i.alias.clone(),
            items: ir_items,
        }))
    }

    /// Whether an imported binding names the stdlib declaration stub of a derivable trait.
    ///
    /// `Clone`, `Copy`, `Default`, `Debug`, `Display`, `Eq`, `Ord`, `Hash` and their partial forms are declared as
    /// Incan traits under `std.derives.*` so that `@derive(...)`, `with` clauses and bounds can spell them, but the
    /// implementation every generated program carries is the Rust trait the derive implements: a `Clone` bound lowers
    /// to Rust's `Clone` (`incan_lang::lang::trait_bounds`), never to the stub (#1374). Importing the stub into a
    /// module therefore binds nothing that generated Rust may use. Lowering it as a `use` would bind the stub under the
    /// Rust trait's own name and shadow the prelude for the whole module, so every bare `Clone` in it -- a written
    /// bound, an inferred one, or the stdlib trait defaults expanded into an adopter's impl -- would name a trait no
    /// derived type implements (#1727). The identity is the checked declaring module (a facade re-export and a
    /// stdlib-provider spelling both resolve to it), matched against the registry of derive-tree stub modules; the
    /// `std` root, whose import binds the builtin trait itself, is the same vocabulary. A same-named local or
    /// third-party trait keeps its import.
    fn is_derive_vocabulary_trait_import(&self, binding_name: &str) -> bool {
        let (Some(declaring_module), Some(source_name)) = self.canonical_trait_identity(binding_name) else {
            return false;
        };
        let Some(stub_module) = stdlib::trait_method_module_segments(&source_name) else {
            return false;
        };
        stub_module == declaring_module
            || matches!(declaring_module.as_slice(), [root] if root.as_str() == stdlib::STDLIB_ROOT)
    }

    /// Return concrete Rust function names needed to import one overload binding.
    fn emitted_overload_import_names(&self, binding_name: &str) -> Option<Vec<String>> {
        let info = self.type_info.as_ref()?;
        if let Some(names) = info.imported_function_emitted_names(binding_name) {
            return Some(names.to_vec());
        }
        let names = info
            .function_overloads(binding_name)?
            .iter()
            .filter_map(|overload| overload.info.emitted_name.clone())
            .collect::<Vec<_>>();
        (!names.is_empty()).then_some(names)
    }

    /// Return the exact overload identity paired with one frontend-selected provider name.
    fn overload_import_identity(
        &self,
        binding_name: &str,
        emitted_name: &str,
    ) -> Option<incan_semantics_core::CanonicalSymbolId> {
        self.type_info
            .as_ref()?
            .function_overloads(binding_name)?
            .iter()
            .find(|overload| overload.info.emitted_name.as_deref() == Some(emitted_name))?
            .identity
            .clone()
    }
}
