//! Imports of Rust items: `import rust::…` and `from rust::… import …`, bound with the Rust metadata the session
//! prepared for them.

use crate::ast::*;
use crate::diagnostics::errors;
use crate::symbols::*;
use crate::typechecker::TypeChecker;
use crate::typechecker::type_info::RustTraitImportInfo;
use incan_lang::interop::{RustItemKind, RustTraitAssoc, fallback_rust_trait_methods};
use std::collections::HashSet;

impl TypeChecker {
    /// Collect a Rust crate or crate-path import and attach metadata when available.
    pub(super) fn collect_rust_crate_import(
        &mut self,
        crate_name: &str,
        path: &[Ident],
        alias: Option<&Ident>,
        span: Span,
    ) {
        if self.reject_unsupported_rust_core_alloc(crate_name, span) {
            return;
        }

        // Rust crate import: `import rust::serde_json` or `import rust::serde_json::Value`.
        let name = alias
            .cloned()
            .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.to_string()));
        let full_path = self.rust_import_full_path(crate_name, path, None);
        let binding = if path.is_empty() {
            RustImportBindingKind::CrateRoot
        } else {
            RustImportBindingKind::RootedPath
        };
        let canonical_path = full_path.join("::");
        let info = RustItemInfo {
            crate_name: crate_name.to_string(),
            path: canonical_path.clone(),
            binding,
            metadata: self.rust_item_metadata_for_path(&canonical_path),
        };
        self.define_rust_import_binding(name, info, span);
    }

    /// Collect `from rust::... import ...` items and attach any already-prepared metadata.
    pub(super) fn collect_rust_from_imports(
        &mut self,
        crate_name: &str,
        path: &[Ident],
        items: &[ImportItem],
        span: Span,
    ) {
        if self.reject_unsupported_rust_core_alloc(crate_name, span) {
            return;
        }

        for item in items {
            let name = Self::import_item_local_name(item);
            let full_path = self.rust_import_full_path(crate_name, path, Some(&item.name));
            let canonical_path = full_path.join("::");
            let info = RustItemInfo {
                crate_name: crate_name.to_string(),
                path: canonical_path.clone(),
                binding: RustImportBindingKind::FromImport,
                metadata: self.rust_item_metadata_for_path(&canonical_path),
            };
            self.define_rust_import_binding(name, info, span);
        }
    }

    /// Emit the RFC 005 diagnostic for unsupported `rust::core` / `rust::alloc` imports.
    ///
    /// Returns `true` when the crate is unsupported and an error was emitted.
    pub(super) fn reject_unsupported_rust_core_alloc(&mut self, crate_name: &str, span: Span) -> bool {
        if crate_name == "core" || crate_name == "alloc" {
            self.errors.push(errors::unsupported_rust_core_alloc(crate_name, span));
            return true;
        }
        false
    }

    /// Build a full Rust import path vector from crate, optional module path, and optional item name.
    pub(super) fn rust_import_full_path(&self, crate_name: &str, path: &[Ident], item: Option<&str>) -> Vec<Ident> {
        let mut full_path = vec![crate_name.to_string()];
        full_path.extend(path.to_vec());
        if let Some(item_name) = item {
            full_path.push(item_name.to_string());
        }
        full_path
    }

    /// Validate and register a Rust import symbol for codegen and RFC 041 provenance.
    pub(super) fn define_rust_import_binding(&mut self, name: Ident, info: RustItemInfo, span: Span) {
        self.validate_root_namespace(&name, span);
        let mut trait_methods = HashSet::new();
        let mut trait_method_signatures = std::collections::HashMap::new();
        if let Some(metadata) = &info.metadata
            && let RustItemKind::Trait(trait_info) = &metadata.kind
        {
            for item in &trait_info.items {
                match item {
                    RustTraitAssoc::Function { name, signature } => {
                        trait_methods.insert(name.clone());
                        trait_method_signatures.insert(name.clone(), signature.clone());
                    }
                    RustTraitAssoc::TypeAlias { .. } | RustTraitAssoc::Constant { .. } => {}
                }
            }
        }
        if trait_methods.is_empty() {
            trait_methods.extend(
                fallback_rust_trait_methods(info.path.as_str())
                    .iter()
                    .map(|method| (*method).to_string()),
            );
        }
        let symbol_id = self.define_rust_import_symbol(name.clone(), info.clone(), span);
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        // An import with a declared method surface is a trait Rust method lookup can use. An import with no
        // metadata at all may still be one: nothing says which methods it provides, so it stays a candidate for
        // every method call that no inspected surface resolves rather than being pruned as unused (#1450). Metadata
        // that names a non-trait item settles the question, and the binding is left out.
        let methods_known = !trait_methods.is_empty();
        if !methods_known && info.metadata.is_some() {
            return;
        }
        self.type_info.rust.trait_imports.insert(
            name.clone(),
            RustTraitImportInfo {
                trait_path: info.path.clone(),
                definition_path: info
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.definition_path.clone()),
                methods: trait_methods,
                methods_known,
                method_signatures: trait_method_signatures,
            },
        );
    }

    /// Define a symbol for a Rust crate import.
    ///
    /// Explicit Rust imports must be allowed to shadow dependency-exported Incan types with the same simple name. This
    /// matters for Rust metadata display types such as `Duration`, where the current module's `from rust::... import
    /// Duration` is the only reliable hint that an unqualified metadata return type means `std::time::Duration`.
    pub(super) fn define_rust_import_symbol(&mut self, name: Ident, info: RustItemInfo, span: Span) -> SymbolId {
        self.symbols.define_import_binding_with_inferred_target(Symbol {
            name,
            kind: SymbolKind::RustItem(info),
            span,
            scope: 0, // Will be set by define()
        })
    }
}
