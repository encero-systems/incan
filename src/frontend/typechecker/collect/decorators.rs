//! Decorator resolution and validation helpers for the first pass.
//!
//! This keeps decorator path resolution and validation logic out of the main  collection flow while preserving RFC 022
//! semantics.

use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::decorator_resolution;
use crate::frontend::diagnostics::errors;
use crate::frontend::symbols::{ResolvedType, SymbolKind, SymbolTable, TypeInfo};
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::decorators::{self, DecoratorId};
use incan_core::lang::derives;

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
    pub(crate) fn validate_decorators(&mut self, decorators: &[Spanned<Decorator>]) {
        for dec in decorators {
            let resolved = resolve_decorator_path(&dec.node, &self.symbols);
            let Some(_id) = decorators::from_segments(&resolved) else {
                let path = if resolved.is_empty() {
                    dec.node.name.clone()
                } else {
                    resolved.join(".")
                };
                self.errors.push(errors::unknown_decorator(path.as_str(), dec.span));
                continue;
            };
        }
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
