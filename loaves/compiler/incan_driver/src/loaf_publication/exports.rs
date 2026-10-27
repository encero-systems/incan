//! A library unit's public names: the entry module's direct public declarations and its `pub from … import`
//! re-exports, resolved to the checked exports they name.

use std::collections::{HashMap, HashSet};

use incan_frontend::ast::{Declaration, ImportKind};
use incan_frontend::library_exports::{CheckedNamedExport, LibraryExportBindingRegistry};
use incan_frontend::module::canonicalize_source_module_segments;
use incan_frontend::{ParsedModule, diagnostics};

/// The module key of canonicalized source path segments.
pub fn module_key(path_segments: &[String]) -> String {
    canonicalize_source_module_segments(path_segments).join("_")
}

/// Resolves a library entry module's public names over checked exports.
pub struct LibraryReexportResolver<'a> {
    /// Checked exports keyed by canonical source-module name, then by source export name.
    pub module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>,
}

impl<'a> LibraryReexportResolver<'a> {
    /// Create a resolver over checked exports grouped by canonical source-module name and source export name.
    pub fn new(module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>) -> Self {
        Self { module_exports }
    }

    /// Resolve direct public declarations and `pub from ... import ...` declarations in a library entrypoint into
    /// checked public exports.
    ///
    /// A single source name can map to several checked exports when the provider exposes same-name overloads. The
    /// resolver therefore preserves all matching exports and only applies the consumer-facing alias to each one.
    pub fn resolve(
        &self,
        lib_module: &ParsedModule,
    ) -> Result<Vec<CheckedNamedExport>, Vec<incan_frontend::diagnostics::CompileError>> {
        let mut errors = Vec::new();
        let mut resolved = Vec::new();
        let mut exported_names = LibraryExportBindingRegistry::default();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();
        let entrypoint_exports = self.module_exports.get(&module_key(&lib_module.path_segments));

        if let Some(exports_by_name) = entrypoint_exports {
            let mut direct_groups = HashSet::new();
            for (export_name, export_span) in Self::direct_public_exports(lib_module) {
                let exports = exports_by_name.get(&export_name);
                // The checked map owns one binding group, including all selected overload declarations. Register
                // that group once; later, distinct import projections still pass through the collision registry.
                if exports.is_some() && !direct_groups.insert(export_name.clone()) {
                    continue;
                }
                if let Err(error) = exported_names.register(&export_name, export_span) {
                    errors.push(error);
                    continue;
                }
                if let Some(exports) = exports {
                    resolved.extend(exports.iter().cloned());
                }
            }
        }

        for decl in &lib_module.ast.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            if !matches!(import.visibility, incan_frontend::ast::Visibility::Public) {
                continue;
            }

            let checked_external_import = match &import.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => Some(("rust", crate_name, path, items)),
                ImportKind::PubFrom { library, path, items } => Some(("pub", library, path, items)),
                _ => None,
            };
            if let Some((namespace, provider, path, items)) = checked_external_import {
                let Some(exports_by_name) = entrypoint_exports else {
                    errors.push(diagnostics::errors::library_reexport_unknown_module(
                        &module_key(&lib_module.path_segments),
                        &known_modules,
                        decl.span,
                    ));
                    continue;
                };
                let mut source_segments = vec![namespace.to_string(), provider.clone()];
                source_segments.extend(path.iter().cloned());
                let source_path = source_segments.join("::");

                for item in items {
                    let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                    if let Err(error) = exported_names.register(&exported_name, decl.span) {
                        errors.push(error);
                        continue;
                    }

                    let Some(exports) = exports_by_name.get(&exported_name) else {
                        let available: Vec<String> = exports_by_name.keys().cloned().collect();
                        errors.push(diagnostics::errors::import_not_exported(
                            &item.name,
                            &source_path,
                            &available,
                            decl.span,
                        ));
                        continue;
                    };
                    resolved.extend(exports.iter().cloned());
                }
                continue;
            }

            let ImportKind::From { module, items } = &import.kind else {
                errors.push(diagnostics::errors::library_pub_reexport_requires_from(decl.span));
                continue;
            };

            let module_name = module_key(&module.segments);
            let Some(exports_by_name) = self.module_exports.get(&module_name) else {
                errors.push(diagnostics::errors::library_reexport_unknown_module(
                    &module.to_rust_path(),
                    &known_modules,
                    decl.span,
                ));
                continue;
            };

            for item in items {
                let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                if let Err(error) = exported_names.register(&exported_name, decl.span) {
                    errors.push(error);
                    continue;
                }

                let Some(exports) = exports_by_name.get(&item.name) else {
                    let available: Vec<String> = exports_by_name.keys().cloned().collect();
                    errors.push(diagnostics::errors::import_not_exported(
                        &item.name,
                        &module.to_rust_path(),
                        &available,
                        decl.span,
                    ));
                    continue;
                };

                resolved.extend(
                    exports
                        .iter()
                        .map(|export| export.projected_through_reexport(&exported_name, entrypoint_exports)),
                );
            }
        }

        if errors.is_empty() { Ok(resolved) } else { Err(errors) }
    }

    /// Return public names declared directly by the library entrypoint, excluding public imports that are resolved from
    /// their source module below.
    pub fn direct_public_exports(lib_module: &ParsedModule) -> Vec<(String, incan_frontend::ast::Span)> {
        lib_module
            .ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Function(function)
                    if matches!(function.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((function.name.clone(), decl.span))
                }
                Declaration::Model(model) if matches!(model.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((model.name.clone(), decl.span))
                }
                Declaration::Class(class) if matches!(class.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((class.name.clone(), decl.span))
                }
                Declaration::Trait(trait_decl)
                    if matches!(trait_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((trait_decl.name.clone(), decl.span))
                }
                Declaration::Enum(enum_decl)
                    if matches!(enum_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((enum_decl.name.clone(), decl.span))
                }
                Declaration::Newtype(newtype_decl)
                    if matches!(newtype_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((newtype_decl.name.clone(), decl.span))
                }
                Declaration::TypeAlias(alias)
                    if matches!(alias.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Const(konst) if matches!(konst.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((konst.name.clone(), decl.span))
                }
                Declaration::Static(static_decl)
                    if matches!(static_decl.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((static_decl.name.clone(), decl.span))
                }
                Declaration::Alias(alias) if matches!(alias.visibility, incan_frontend::ast::Visibility::Public) => {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Partial(partial)
                    if matches!(partial.visibility, incan_frontend::ast::Visibility::Public) =>
                {
                    Some((partial.name.clone(), decl.span))
                }
                _ => None,
            })
            .collect()
    }
}
