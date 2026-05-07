//! Decorator resolution and validation helpers for the first pass.
//!
//! This keeps decorator path resolution and validation logic out of the main collection flow while preserving RFC 022
//! semantics.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, SymbolKind, SymbolTable, TypeInfo};
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives;
use incan_semantics_core::{DecoratorFeature, SurfaceFeatureKey};

/// Resolve a decorator path to a module path.
pub(in crate::frontend::typechecker) fn resolve_decorator_path(dec: &Decorator, symbols: &SymbolTable) -> Vec<String> {
    decorator_resolution::resolve_decorator_path(dec, symbols)
}

/// Resolve a decorator path to a decorator id.
pub(in crate::frontend::typechecker) fn resolve_decorator_id(
    dec: &Decorator,
    symbols: &SymbolTable,
) -> Option<DecoratorId> {
    let resolved = resolve_decorator_path(dec, symbols);
    decorators::from_segments(&resolved)
}

/// Find decorators by name.
pub(super) fn decorators_named<'a>(
    decorators: &'a [Spanned<Decorator>],
    symbols: &SymbolTable,
    id: DecoratorId,
) -> impl Iterator<Item = &'a Spanned<Decorator>> {
    decorators
        .iter()
        .filter(move |d| resolve_decorator_id(&d.node, symbols) == Some(id))
}

/// Extract positional identifier names from decorator arguments.
pub(super) fn positional_idents(args: &[DecoratorArg]) -> impl Iterator<Item = (&str, Span)> + '_ {
    args.iter().filter_map(|arg| match arg {
        DecoratorArg::Positional(expr) => {
            if let Expr::Ident(name) = &expr.node {
                Some((name.as_str(), expr.span))
            } else {
                None
            }
        }
        _ => None,
    })
}

impl TypeChecker {
    /// Validate decorator paths.
    ///
    /// When a decorator doesn't resolve to a known `DecoratorId`, the error message is contextual:
    /// - If the leading segment is a known namespace (e.g. `rust`, `std`), the error mentions the namespace and lists
    ///   available decorators within it.
    /// - Otherwise, a generic "unknown decorator" error is emitted.
    pub(crate) fn validate_decorators(&mut self, decorators: &[Spanned<Decorator>]) {
        for dec in decorators {
            let mut resolved = resolve_decorator_path(&dec.node, &self.symbols);
            let mut feature = self.surface_context.decorator_feature_for_path(&resolved);

            // ---- Fallback: import-alias resolution ----
            // The SymbolTable-based `DecoratorPrefixLookup` only handles Module symbols, so decorators imported as
            // functions (e.g. `from std.testing import parametrize` then `@parametrize(...)`) won't resolve via the
            // symbol table. Fall back to the import aliases collected from the program's `import` / `from ... import`
            // declarations, which correctly map `parametrize` → `["std", "testing", "parametrize"]`.
            if feature.is_none() && decorators::from_segments(&resolved).is_none() {
                let alias_resolved = decorator_resolution::resolve_decorator_path(&dec.node, &self.import_aliases);
                if alias_resolved != resolved {
                    resolved = alias_resolved;
                    feature = self.surface_context.decorator_feature_for_path(&resolved);
                }
            }

            let Some(id) = decorators::from_segments(&resolved) else {
                let is_stdlib_decorator_function = feature
                    == Some(SurfaceFeatureKey::Decorator(DecoratorFeature::StdlibDecoratorFunction))
                    && resolved.len() >= 3
                    && self
                        .stdlib_cache
                        .lookup_function_meta(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
                        .is_some_and(|f| f.is_rust_extern && f.rust_module_path.is_some());
                if is_stdlib_decorator_function {
                    continue;
                }

                let path = if resolved.is_empty() {
                    dec.node.name.clone()
                } else {
                    resolved.join(".")
                };

                // ---- Namespace-aware error (e.g. "@rust.blah" → "unknown in `rust` namespace") ----
                if let Some(first) = resolved.first()
                    && decorators::is_known_decorator_namespace(first)
                {
                    let known = decorators::decorators_in_namespace(first);
                    let known_display: Vec<_> = known.iter().map(|d| format!("@{d}")).collect();
                    let hint = if known_display.is_empty() {
                        format!("No decorators are currently defined in the `{first}` namespace")
                    } else {
                        format!("Known `{first}` decorators: {}", known_display.join(", "))
                    };
                    self.errors
                        .push(errors::unknown_decorator(&path, dec.span).with_hint(&hint));
                } else {
                    self.errors.push(errors::unknown_decorator(&path, dec.span));
                }
                continue;
            };

            if id == DecoratorId::RustAllow {
                self.validate_rust_allow_args(dec);
            }
        }
    }

    /// Validate RFC 057 `@rust.allow(...)` arguments.
    ///
    /// The decorator is intentionally item-scoped and accepts only explicit lint paths so generated code can emit
    /// targeted `#[allow(...)]` attributes without introducing broad crate- or module-level suppression.
    pub(crate) fn validate_rust_allow_args(&mut self, dec: &Spanned<Decorator>) {
        let mut seen = HashSet::new();
        let mut positional_count = 0usize;

        for arg in &dec.node.args {
            match arg {
                DecoratorArg::Positional(expr) => {
                    positional_count += 1;
                    let Expr::Literal(Literal::String(name)) = &expr.node else {
                        self.errors
                            .push(errors::rust_allow_requires_positional_string(expr.span));
                        continue;
                    };
                    self.validate_single_rust_allow_lint(name, expr.span, &mut seen);
                }
                DecoratorArg::Named(name, _) => {
                    self.errors.push(errors::rust_allow_rejects_named_args(name, dec.span));
                }
            }
        }

        if positional_count == 0 {
            self.errors
                .push(errors::rust_allow_requires_positional_string(dec.span));
        }
    }

    /// Reject RFC 057 `@rust.allow(...)` on declarations that do not own a supported Rust item boundary.
    ///
    /// Parser syntax allows decorators on several declaration forms. This helper keeps the semantic support matrix
    /// explicit so adding a new declaration kind does not silently inherit Rust lint suppression behavior.
    pub(crate) fn reject_rust_allow_on_unsupported_declaration(
        &mut self,
        decorators: &[Spanned<Decorator>],
        kind: &'static str,
    ) {
        for dec in decorators {
            if self.decorator_id_with_import_aliases(&dec.node) == Some(DecoratorId::RustAllow) {
                self.errors
                    .push(errors::rust_allow_unsupported_attachment(kind, dec.span));
            }
        }
    }

    fn decorator_id_with_import_aliases(&self, dec: &Decorator) -> Option<DecoratorId> {
        let resolved = resolve_decorator_path(dec, &self.symbols);
        if let Some(id) = decorators::from_segments(&resolved) {
            return Some(id);
        }

        let alias_resolved = decorator_resolution::resolve_decorator_path(dec, &self.import_aliases);
        decorators::from_segments(&alias_resolved)
    }

    fn validate_single_rust_allow_lint(&mut self, name: &str, span: Span, seen: &mut HashSet<String>) {
        if name.is_empty() || name.trim() != name || !Self::is_valid_rust_lint_path(name) {
            self.errors.push(errors::rust_allow_invalid_lint_name(name, span));
            return;
        }

        if Self::is_broad_rust_lint_group(name) {
            self.errors.push(errors::rust_allow_broad_lint_group(name, span));
            return;
        }

        if !seen.insert(name.to_string()) {
            self.errors.push(errors::rust_allow_duplicate_lint(name, span));
        }
    }

    fn is_valid_rust_lint_path(name: &str) -> bool {
        name.split("::").all(Self::is_valid_rust_lint_segment)
    }

    fn is_valid_rust_lint_segment(segment: &str) -> bool {
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    fn is_broad_rust_lint_group(name: &str) -> bool {
        matches!(
            name,
            "warnings"
                | "unused"
                | "clippy::all"
                | "clippy::pedantic"
                | "clippy::nursery"
                | "clippy::restriction"
                | "clippy::cargo"
        )
    }

    /// Validate @derive decorator arguments and report errors for unknown derives.
    pub(crate) fn validate_derives(&mut self, decorators: &[Spanned<Decorator>]) {
        let derive_items: Vec<_> = decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| {
                dec.node.args.iter().filter_map(|arg| match arg {
                    DecoratorArg::Positional(expr) => {
                        if let Expr::Ident(name) = &expr.node {
                            Some((name.clone(), expr.span))
                        } else {
                            None
                        }
                    }
                    DecoratorArg::Named(name, _) => {
                        // Named args not valid for derive, but report error on them.
                        Some((name.clone(), dec.span))
                    }
                })
            })
            .collect();

        for (name, span) in derive_items {
            self.validate_single_derive(&name, span);
        }
    }

    /// Extract derive names from @derive decorators.
    pub(crate) fn extract_derive_names(&self, decorators: &[Spanned<Decorator>]) -> Vec<String> {
        decorators_named(decorators, &self.symbols, DecoratorId::Derive)
            .flat_map(|dec| positional_idents(&dec.node.args))
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Extract `@requires` constraints from decorators as `(name, type)` pairs.
    pub(super) fn extract_requires(&mut self, decorators: &[Spanned<Decorator>]) -> Vec<(String, ResolvedType)> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut requires: Vec<(String, ResolvedType)> = Vec::new();

        for dec in decorators {
            if resolve_decorator_id(&dec.node, &self.symbols) != Some(DecoratorId::Requires) {
                continue;
            }
            for arg in &dec.node.args {
                if let DecoratorArg::Named(name, DecoratorArgValue::Type(ty)) = arg {
                    if !seen.insert(name.clone()) {
                        self.errors.push(errors::duplicate_trait_requires_field(name, ty.span));
                        continue;
                    }
                    requires.push((name.clone(), self.resolve_type_checked(ty)));
                }
            }
        }
        requires
    }

    /// Validate a single derive name, reporting appropriate errors.
    fn validate_single_derive(&mut self, name: &str, span: Span) {
        if derives::from_str(name).is_some() {
            return;
        }

        if self
            .lookup_symbol(name)
            .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Module(_)))
            && let Some(module_path) = self.module_path_for_imported_name(name)
        {
            if self.lookup_derivable_traits(&module_path).is_some() {
                return;
            }
            self.errors.push(errors::derive_module_missing_derives(name, span));
            return;
        }

        if let Some((canonical, info)) = self.resolve_qualified_trait(name) {
            self.define_hidden_trait_symbol(&canonical, info, span);
            return;
        }

        // Allow custom derives imported from stdlib modules backed by rust.module(...).
        let resolved = self
            .import_aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| vec![name.to_string()]);
        if resolved.len() >= 2
            && self.imported_trait_is_derivable(&resolved[..resolved.len() - 1], &resolved[resolved.len() - 1])
        {
            return;
        }

        // Check if the name refers to a type/function (wrong usage)
        if let Some(kind_name) = self.lookup_symbol_kind(name) {
            self.errors.push(errors::derive_wrong_kind(name, kind_name, span));
        } else {
            self.errors.push(errors::unknown_derive(name, span));
        }
    }

    /// Look up what kind of symbol a name refers to, if any.
    fn lookup_symbol_kind(&self, name: &str) -> Option<&'static str> {
        let sym_id = self.symbols.lookup(name)?;
        let sym = self.symbols.get(sym_id)?;

        match &sym.kind {
            SymbolKind::Type(TypeInfo::Model(_)) => Some("model"),
            SymbolKind::Type(TypeInfo::Class(_)) => Some("class"),
            SymbolKind::Type(TypeInfo::Enum(_)) => Some("enum"),
            SymbolKind::Function(_) => Some("function"),
            _ => None,
        }
    }
}
