//! Import declaration lowering.

use super::super::super::decl::{IrDeclKind, IrRustTraitImport};
use super::super::AstLowering;
use crate::frontend::ast;
use crate::frontend::module::canonicalize_source_module_segments;

impl AstLowering {
    /// Lower an import declaration.
    pub(in crate::backend::ir::lower) fn lower_import(&self, i: &ast::ImportDecl) -> IrDeclKind {
        let (path, ast_items) = match &i.kind {
            ast::ImportKind::Module(p) => (canonicalize_source_module_segments(&p.segments), vec![]),
            ast::ImportKind::From { module, items } => {
                (canonicalize_source_module_segments(&module.segments), items.clone())
            }
            ast::ImportKind::PubLibrary { library } => (vec![library.clone()], vec![]),
            ast::ImportKind::PubFrom { library, items } => (vec![library.clone()], items.clone()),
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
            ast::ImportKind::PubLibrary { library } | ast::ImportKind::PubFrom { library, .. } => {
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
            .map(|item| {
                let binding_name = item.alias.as_ref().unwrap_or(&item.name);
                let rust_trait_import = self
                    .type_info
                    .as_ref()
                    .and_then(|info| info.rust_trait_imports.get(binding_name))
                    .map(|import| {
                        let mut methods: Vec<_> = import.methods.iter().cloned().collect();
                        methods.sort();
                        IrRustTraitImport {
                            trait_path: import.trait_path.clone(),
                            definition_path: import.definition_path.clone(),
                            methods,
                        }
                    });
                super::super::super::decl::IrImportItem {
                    name: item.name.clone(),
                    alias: item.alias.clone(),
                    rust_trait_import,
                }
            })
            .collect();

        IrDeclKind::Import {
            visibility: Self::map_visibility(i.visibility),
            origin,
            qualifier,
            path,
            alias: i.alias.clone(),
            items: ir_items,
        }
    }
}
