//! Import declaration lowering.

use super::super::super::decl::{IrDeclKind, IrRustTraitImport};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use incan_frontend::ast;
use incan_frontend::module::canonicalize_source_module_segments;
use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

impl AstLowering {
    /// Lower an import declaration.
    pub(in crate::lower) fn lower_import(
        &self,
        i: &ast::ImportDecl,
        span: ast::Span,
    ) -> Result<IrDeclKind, LoweringError> {
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
                let canonical = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.resolved_import_identity(binding_name))
                    .cloned();
                let (name, alias) = self.projected_import_spelling(item, canonical.as_ref());
                vec![super::super::super::decl::IrImportItem {
                    name,
                    alias,
                    canonical,
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

        Ok(IrDeclKind::Import {
            visibility: Self::map_visibility(i.visibility),
            origin,
            qualifier,
            path,
            alias: i.alias.clone(),
            items: ir_items,
        })
    }

    /// Spell a projected source import by the name its declaring module binds the declaration under.
    ///
    /// A projected symbol (function, partial, static) is spelled by its identity projection in every module; the
    /// written item name never reaches the Rust. What the item's `name` still decides is whether this module binds
    /// the projection itself: an item whose name is the declaration's own spelling does, while an `alias` declaration
    /// (`pub scale_alias = alias scale`) is a second public name over a projection the declaration's own import binds.
    /// A facade re-export under a new name (`pub from provider import calculate as facade_calculate`) is neither: a
    /// consumer of `facade_calculate` binds the projection exactly as a direct import of `calculate` would, so the
    /// item is spelled `calculate` and the written name becomes the alias this module exposes it under, the shape a
    /// single-level `pub from … import … as …` already lowers to (#1710). Types and traits keep the written spelling:
    /// their Rust name is the spelling, and a renamed facade type must stay reachable under its facade name.
    fn projected_import_spelling(
        &self,
        item: &ast::ImportItem,
        canonical: Option<&incan_semantics_core::CanonicalSymbolId>,
    ) -> (String, Option<String>) {
        let written = (item.name.clone(), item.alias.clone());
        if !canonical.is_some_and(crate::decl::is_projected_source_symbol) {
            return written;
        }
        let binding_name = item.alias.as_deref().unwrap_or(&item.name);
        let Some(declared_name) = self
            .type_info
            .as_ref()
            .and_then(|info| info.resolved_import_declared_name(binding_name))
            .filter(|declared_name| *declared_name != item.name)
        else {
            return written;
        };
        let exposed_as = item.alias.clone().unwrap_or_else(|| item.name.clone());
        (declared_name.to_string(), Some(exposed_as))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decl::IrImportItem;
    use incan_frontend::ast::Program;
    use incan_frontend::{lexer, parser, typechecker::TypeChecker};

    /// Parse one module and keep fixture failures as ordinary test errors.
    fn parse(source: &str, context: &str) -> Result<Program, String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("{context} lex failed: {errors:?}"))?;
        parser::parse(&tokens).map_err(|errors| format!("{context} parse failed: {errors:?}"))
    }

    /// Check `module` against its dependencies and return the import items its lowering records.
    fn lowered_import_items(
        module_name: &str,
        module: &Program,
        dependencies: &[(&str, &Program)],
    ) -> Result<Vec<IrImportItem>, String> {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec![module_name.to_string()]));
        checker
            .check_with_imports(module, dependencies)
            .map_err(|errors| format!("{module_name} should typecheck: {errors:?}"))?;
        let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
        let ir = lowering
            .lower_program(module)
            .map_err(|errors| format!("{module_name} lowering failed: {errors:?}"))?;
        Ok(ir
            .declarations
            .into_iter()
            .filter_map(|decl| match decl.kind {
                IrDeclKind::Import { items, .. } => Some(items),
                _ => None,
            })
            .flatten()
            .collect())
    }

    /// Return the one import item a module lowers, by the local name it binds.
    fn item_bound_as(items: &[IrImportItem], local_name: &str) -> Result<IrImportItem, String> {
        items
            .iter()
            .find(|item| item.source_binding_name() == local_name)
            .cloned()
            .ok_or_else(|| format!("no import item binds `{local_name}`: {items:?}"))
    }

    /// The second hop of a renamed re-export chain lowers exactly like the first hop (#1710).
    ///
    /// `public_api` imports `facade_calculate`, the facade's rename of `provider.calculate`. Spelled as written, the
    /// item read as an `alias` declaration and the module bound only the alias name, never the projection the next
    /// module imports. Spelled by the declaring module's name with the written name as the alias, it is the shape a
    /// single-level `pub from provider import calculate as facade_calculate` lowers to.
    #[test]
    fn renamed_reexport_hop_is_spelled_by_its_declaring_module_issue1710() -> Result<(), String> {
        let provider = parse(
            "pub def calculate(value: int) -> int:\n  return value + 1\n",
            "provider",
        )?;
        let facade = parse("pub from provider import calculate as facade_calculate\n", "facade")?;
        let public_api = parse(
            "pub from facade import facade_calculate as exported_calculate\n",
            "public_api",
        )?;

        let facade_items = lowered_import_items("facade", &facade, &[("provider", &provider)])?;
        let first_hop = item_bound_as(&facade_items, "facade_calculate")?;
        assert_eq!(first_hop.name, "calculate");
        assert_eq!(first_hop.alias.as_deref(), Some("facade_calculate"));

        let public_api_items = lowered_import_items(
            "public_api",
            &public_api,
            &[("provider", &provider), ("facade", &facade)],
        )?;
        let second_hop = item_bound_as(&public_api_items, "exported_calculate")?;
        assert_eq!(
            second_hop.name, "calculate",
            "the hop is spelled by the name the provider binds the declaration under"
        );
        assert_eq!(second_hop.alias.as_deref(), Some("exported_calculate"));
        assert_eq!(
            second_hop
                .canonical
                .as_ref()
                .map(|identity| identity.declaration_name.as_str()),
            Some("calculate"),
            "the identity stays the provider's"
        );
        assert_eq!(
            second_hop.emitted_name(),
            first_hop.emitted_name(),
            "both hops bind one projection"
        );

        let consumer = parse(
            "from public_api import exported_calculate\n\ndef run() -> int:\n  return exported_calculate(41)\n",
            "consumer",
        )?;
        let consumer_items = lowered_import_items(
            "consumer",
            &consumer,
            &[
                ("provider", &provider),
                ("facade", &facade),
                ("public_api", &public_api),
            ],
        )?;
        let unaliased = item_bound_as(&consumer_items, "exported_calculate")?;
        assert_eq!(unaliased.name, "calculate");
        assert_eq!(
            unaliased.alias.as_deref(),
            Some("exported_calculate"),
            "an import written without an alias still exposes the written name, never the declaring one"
        );
        Ok(())
    }

    /// An imported `alias` declaration keeps its own spelling: the declaring module binds it under that name.
    #[test]
    fn imported_alias_declaration_keeps_its_spelling_issue1710() -> Result<(), String> {
        let provider = parse(
            "pub def scale(value: int) -> int:\n  return value * 2\n\npub scale_alias = alias scale\n",
            "provider",
        )?;
        let consumer = parse(
            "from provider import scale, scale_alias\n\ndef run() -> int:\n  return scale(1) + scale_alias(2)\n",
            "consumer",
        )?;
        let items = lowered_import_items("consumer", &consumer, &[("provider", &provider)])?;
        let alias = item_bound_as(&items, "scale_alias")?;
        assert_eq!(alias.name, "scale_alias");
        assert_eq!(alias.alias, None);
        assert_eq!(
            alias
                .canonical
                .as_ref()
                .map(|identity| identity.declaration_name.as_str()),
            Some("scale")
        );
        Ok(())
    }
}
