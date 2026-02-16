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
use incan_core::lang::stdlib;

/// Resolve a decorator path to a module path.
pub(super) fn resolve_decorator_path(dec: &Decorator, symbols: &SymbolTable) -> Vec<String> {
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

            // ---- Fallback: import-alias resolution ----
            // The SymbolTable-based `DecoratorPrefixLookup` only handles Module symbols, so decorators imported as
            // functions (e.g. `from std.testing import parametrize` then `@parametrize(...)`) won't resolve via the
            // symbol table. Fall back to the import aliases collected from the program's `import` / `from ... import`
            // declarations, which correctly map `parametrize` → `["std", "testing", "parametrize"]`.
            if decorators::from_segments(&resolved).is_none() && !self.is_stdlib_decorator_function(&resolved) {
                let alias_resolved = decorator_resolution::resolve_decorator_path(&dec.node, &self.import_aliases);
                if alias_resolved != resolved {
                    resolved = alias_resolved;
                }
            }

            let Some(_id) = decorators::from_segments(&resolved) else {
                if self.is_stdlib_decorator_function(&resolved) {
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
        }
    }

    /// Allow stdlib functions to be used as decorators without requiring core-registry entries.
    ///
    /// This keeps user-facing stdlib surface (like `std.testing.fixture`) out of `incan_core`.
    fn is_stdlib_decorator_function(&mut self, resolved: &[String]) -> bool {
        if resolved.len() < 3 || !matches!(resolved.first(), Some(seg) if seg == stdlib::STDLIB_ROOT) {
            return false;
        }

        let (module_path, function_name) = resolved.split_at(resolved.len() - 1);
        let Some(function_name) = function_name.first() else {
            return false;
        };

        if !stdlib::is_known_stdlib_module(module_path) {
            return false;
        }

        self.stdlib_cache.lookup_function(module_path, function_name).is_some()
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
