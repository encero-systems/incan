//! Stdlib-aware import collection and namespace validation.
//!
//! This keeps stdlib import enforcement (RFC 022) separate from general declaration
//! collection while preserving the existing behavior.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::diagnostics::errors;
use crate::frontend::module::ExportedSymbol;
use crate::frontend::symbols::*;
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::stdlib;
use incan_core::lang::surface::types as surface_types;

use super::{stdlib_async, stdlib_testing};

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
                let name = import
                    .alias
                    .clone()
                    .unwrap_or_else(|| path.segments.last().cloned().unwrap_or_else(|| "module".to_string()));
                // Allow `import std.web as std` (alias matches source root), but
                // reject `import std.web as rust` (alias is a different reserved root).
                let same_root = path.segments.first().map(|s| s.as_str()) == Some(&name);
                if !same_root {
                    self.validate_root_namespace(&name, span);
                }
                self.define_import_symbol(name, path.segments.clone(), false, span);
            }
            ImportKind::From { module, items } => {
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
                let module_path_str = module.segments.join(".");

                // Special-case stdlib testing API:
                // `from std.testing import assert_eq, ...` should work as normal function imports (LSP/typechecker),
                // while backend codegen maps these to `incan_stdlib::testing::*`.
                if module.parent_levels == 0
                    && !module.is_absolute
                    && module.segments == vec![stdlib::STDLIB_ROOT.to_string(), stdlib::STDLIB_TESTING.to_string()]
                {
                    for item in items {
                        let local_name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                        self.validate_root_namespace(&local_name, span);
                        if let Some(info) = stdlib_testing::testing_import_function_info(&item.name) {
                            self.symbols.define(Symbol {
                                name: local_name,
                                kind: SymbolKind::Function(info),
                                span,
                                scope: 0,
                            });
                        } else {
                            let mut path = module.segments.clone();
                            path.push(item.name.clone());
                            self.define_import_symbol(local_name, path, false, span);
                        }
                    }
                    return;
                }

                // For each item in `from module import item1, item2, ...`
                // create a symbol as if it were `import module::item`
                for item in items {
                    // Stdlib-scoped surface types: define them as builtin types only when imported from their owning
                    // module.
                    if let Some(id) = surface_types::from_str(item.name.as_str()) {
                        if let Some(expected_module_path) = surface_types::stdlib_module_path(id) {
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
                    }

                    // Stdlib async helper functions become available when explicitly imported.
                    if is_std_async {
                        if let Some((info, expected_module)) = stdlib_async::async_import_function_info(&item.name) {
                            if is_async_prelude || std_async_submodule == Some(expected_module) {
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
            ImportKind::RustCrate { crate_name, path } => {
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
        if let Some(id) = self.symbols.lookup(&name) {
            if let Some(sym) = self.symbols.get(id) {
                match &sym.kind {
                    SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_) => {
                        return;
                    }
                    _ => {}
                }
            }
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
        if let Some(id) = self.symbols.lookup(&name) {
            if let Some(sym) = self.symbols.get(id) {
                match &sym.kind {
                    SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::Trait(_) | SymbolKind::Variant(_) => {
                        // Already have a real definition, don't overwrite with Module placeholder
                        return;
                    }
                    _ => {}
                }
            }
        }

        self.symbols.define(Symbol {
            name,
            kind: SymbolKind::Module(ModuleInfo { path, is_python }),
            span,
            scope: 0,
        });
    }
}
