//! Stdlib-aware import collection and namespace validation.
//!
//! This keeps stdlib import enforcement (RFC 022) separate from general declaration collection while preserving the
//! existing behavior.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::module::ExportedSymbol;
use crate::frontend::symbols::*;
use crate::frontend::testing_markers::load_testing_marker_semantics;
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::stdlib;
use incan_core::lang::surface::types as surface_types;
use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

use super::stdlib_async;

impl TypeChecker {
    /// Reject names that shadow reserved root namespaces.
    pub(super) fn validate_root_namespace(&mut self, name: &str, span: Span) {
        if name == stdlib::STDLIB_ROOT || name == "rust" {
            self.errors.push(errors::reserved_root_namespace(name, span));
        }
    }

    /// Register an import declaration in the symbol table.
    pub(super) fn collect_import(&mut self, import: &ImportDecl, span: Span) {
        self.validate_import_visibility(import, span);
        match &import.kind {
            ImportKind::Module(path) => {
                // Reject `import std.f64.consts` — unknown stdlib module; suggest `import rust::std::f64::consts`.
                if stdlib::is_any_stdlib_path(&path.segments)
                    && !stdlib::is_known_stdlib_module(&path.segments)
                {
                    self.errors
                        .push(errors::unknown_stdlib_module(&path.segments.join("."), span));
                }
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.segments.last().cloned().unwrap_or_else(|| "module".to_string()));
                // Allow `import std.web as std` (alias matches source root), but reject `import std.web as rust` (alias is a different reserved root).
                let same_root = path.segments.first().map(|s| s.as_str()) == Some(&name);
                if !same_root {
                    self.validate_root_namespace(&name, span);
                }
                self.define_import_symbol(name, path.segments.clone(), false, span);
            }
            ImportKind::From { module, items } => {
                // Reject unknown stdlib module, e.g. `from std.f64.consts import PI`;
                // suggest a correction, e.g.`from rust::std::f64::consts import PI`.
                if module.parent_levels == 0
                    && !module.is_absolute
                    && stdlib::is_any_stdlib_path(&module.segments)
                    && !stdlib::is_known_stdlib_module(&module.segments)
                {
                    self.errors
                        .push(errors::unknown_stdlib_module(&module.segments.join("."), span));
                }

                let is_std_web = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() >= 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == stdlib::STDLIB_WEB;
                let is_std_async = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() >= 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "async";
                let std_async_submodule = if is_std_async && module.segments.len() >= 3 {
                    module.segments.get(2).map(|s| s.as_str())
                } else {
                    None
                };
                let is_async_prelude =
                    is_std_async && (module.segments.len() == 2 || std_async_submodule == Some("prelude"));
                let is_std_reflection = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() == 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "reflection";
                let is_std_testing = module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments.len() == 2
                    && module.segments[0] == stdlib::STDLIB_ROOT
                    && module.segments[1] == "testing";
                let is_known_stdlib_with_stub = module.parent_levels == 0
                    && !module.is_absolute
                    && stdlib::is_known_stdlib_module(&module.segments)
                    && stdlib::stdlib_stub_path(&module.segments).is_some();
                let module_path_str = module.segments.join(".");
                let testing_semantics = if is_std_testing {
                    match load_testing_marker_semantics() {
                        Ok(semantics) => Some(semantics),
                        Err(err) => {
                            self.errors
                                .push(errors::invalid_std_testing_marker_metadata(&err.to_string(), span));
                            None
                        }
                    }
                } else {
                    None
                };

                // For each item in `from module import item1, item2, ...`
                // create a symbol as if it were `import module::item`
                for item in items {
                    // Stdlib-scoped surface types: define them as builtin types only when imported from their owning
                    // module.
                    if let Some(id) = surface_types::from_str(item.name.as_str())
                        && let Some(expected_module_path) = surface_types::stdlib_module_path(id) {
                            let allow = match expected_module_path {
                                "std.web" => is_std_web,
                                "std.reflection" => is_std_reflection,
                                _ if expected_module_path.starts_with("std.async.") => {
                                    is_std_async && (is_async_prelude || module_path_str == expected_module_path)
                                }
                                _ => false,
                            };
                            if allow {
                                let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                                self.validate_root_namespace(&local_name, span);
                                self.symbols.define(Symbol {
                                    name: local_name,
                                    kind: SymbolKind::Type(TypeInfo::Builtin),
                                    span,
                                    scope: 0,
                                });
                                continue;
                            }
                        }

                    // RFC 023: for known stdlib modules with `.incn` stubs, prefer AST-derived function signatures.
                    // Async prelude still resolves via dedicated fallback logic below.
                    if is_known_stdlib_with_stub && !is_async_prelude {
                        let ast_info = self.stdlib_cache.lookup_function(&module.segments, &item.name);
                        if let Some(info) = ast_info {
                            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            let mut resolved_marker_path = module.segments.clone();
                            resolved_marker_path.push(item.name.clone());
                            let module_feature = self.surface_context.decorator_feature_for_path(&resolved_marker_path);
                            let marker_feature =
                                testing_semantics
                                    .as_ref()
                                    .and_then(|semantics| semantics.marker_kind(&item.name))
                                    .map(|_| SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker));
                            if is_std_testing
                                && module_feature
                                    == Some(SurfaceFeatureKey::Decorator(
                                        DecoratorFeature::StdlibDecoratorFunction,
                                    ))
                                && marker_feature
                                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::TestingMarker))
                            {
                                self.testing_marker_import_bindings.insert(local_name.clone());
                            }
                            self.validate_root_namespace(&local_name, span);
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Function(info),
                                span,
                                scope: 0,
                            });
                            continue;
                        }
                    }

                    // Stdlib async helper functions become available when explicitly imported.
                    // RFC 023: try AST-derived signatures first (from the parsed .incn files), fall back to hardcoded async_import_function_info().
                    if is_std_async {
                        let ast_info = if is_async_prelude {
                            // Prelude import: search all async submodules for the function.
                            None
                        } else {
                            // Direct submodule import (e.g. `from std.async.time import sleep`).
                            self.stdlib_cache.lookup_function(&module.segments, &item.name)
                        };

                        let resolved = ast_info.map(|info| (info, true)).or_else(|| {
                            stdlib_async::async_import_function_info(&item.name).and_then(|(info, expected_module)| {
                                if is_async_prelude || std_async_submodule == Some(expected_module) {
                                    Some((info, true))
                                } else {
                                    None
                                }
                            })
                        });

                        if let Some((info, _)) = resolved {
                            let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            self.validate_root_namespace(&local_name, span);
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Function(info),
                                span,
                                scope: 0,
                            });
                            continue;
                        }
                    }
                    let aliased_type = item.alias.as_ref().and_then(|alias| {
                        if self.symbols.lookup(alias).is_some() {
                            return None;
                        }
                        let id = self.symbols.lookup(&item.name)?;
                        let sym = self.symbols.get(id)?;
                        let SymbolKind::Type(info) = &sym.kind else {
                            return None;
                        };
                        Some((alias.clone(), info.clone()))
                    });

                    if let Some((alias, info)) = aliased_type {
                        self.symbols.define(Symbol {
                            name: alias,
                            kind: SymbolKind::Type(info),
                            span,
                            scope: 0,
                        });
                        continue;
                    }
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    self.validate_root_namespace(&name, span);
                    let mut path = module.segments.clone();
                    path.push(item.name.clone());
                    self.define_import_symbol(name, path, false, span);
                }
            }
            ImportKind::Python(pkg) => {
                let name = import.alias.clone().unwrap_or_else(|| pkg.clone());
                self.validate_root_namespace(&name, span);
                self.define_import_symbol(name, vec![pkg.clone()], true, span);
            }
            ImportKind::RustCrate { crate_name, path, .. } => {
                // Rust crate import: import rust::serde_json or import rust::serde_json::Value
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.clone()));
                self.validate_root_namespace(&name, span);
                let mut full_path = vec![crate_name.clone()];
                full_path.extend(path.clone());
                // Mark as "rust" import type for codegen
                self.define_rust_import_symbol(name, crate_name.clone(), full_path, span);
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..  // version, features: not used here
            } => {
                // from rust::time import Instant, Duration
                for item in items {
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    self.validate_root_namespace(&name, span);
                    let mut full_path = vec![crate_name.clone()];
                    full_path.extend(path.clone());
                    full_path.push(item.name.clone());
                    self.define_rust_import_symbol(name, crate_name.clone(), full_path, span);
                }
            }
        }
    }

    /// Ensure imported items are public in the dependency module.
    fn validate_import_visibility(&mut self, import: &ImportDecl, span: Span) {
        let ImportKind::From { module, items } = &import.kind else {
            return;
        };

        // Only check modules that were pre-imported; skip std and unresolved ones.
        let module_name = module.segments.join("_");
        let Some(exports) = self.dependency_exports.get(&module_name) else {
            return;
        };

        let mut exported_names: HashSet<String> = HashSet::new();
        for sym in exports {
            match sym {
                ExportedSymbol::Const(name)
                | ExportedSymbol::Type(name)
                | ExportedSymbol::Trait(name)
                | ExportedSymbol::Function(name) => {
                    exported_names.insert(name.clone());
                }
                ExportedSymbol::Variant { variant_name, .. } => {
                    exported_names.insert(variant_name.clone());
                }
            }
        }

        let exported_list: Vec<String> = exported_names.iter().cloned().collect();

        for item in items {
            if !exported_names.contains(&item.name) {
                self.errors.push(errors::import_not_exported(
                    &item.name,
                    &module.to_rust_path(),
                    &exported_list,
                    span,
                ));
            }
        }
    }

    /// Define a symbol for a Rust crate import, skipping if a real definition exists.
    fn define_rust_import_symbol(&mut self, name: Ident, crate_name: String, path: Vec<Ident>, span: Span) {
        if self.has_real_definition(&name) {
            return;
        }
        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::RustModule {
                crate_name,
                path: path.join("::"),
            },
            span,
            scope: 0, // Will be set by define()
        });
    }

    /// Define a symbol for a module import, skipping if a real definition exists.
    fn define_import_symbol(&mut self, name: Ident, path: Vec<Ident>, is_python: bool, span: Span) {
        if self.has_real_definition(&name) {
            return;
        }
        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::Module(ModuleInfo { path, is_python }),
            span,
            scope: 0,
        });
    }

    /// Returns `true` if `name` already resolves to a "real" definition (type, function, trait, or variant) that
    /// should not be overwritten by a module/rust-module placeholder.
    fn has_real_definition(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|sym| {
            matches!(
                sym.kind,
                SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_)
            )
        })
    }
}
