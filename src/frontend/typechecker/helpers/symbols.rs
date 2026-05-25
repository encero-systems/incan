//! Shared symbol-resolution predicates used across typechecker phases.
//!
//! These helpers keep implicit-root-builtin detection in one place so import collection and
//! expression checking make the same shadowing decision.

use crate::frontend::ast::Span;
use crate::frontend::symbols::Symbol;
use crate::frontend::typechecker::TypeChecker;
use incan_core::lang::surface::functions::SurfaceFnId;
use incan_core::lang::surface::types::SurfaceTypeId;

impl TypeChecker {
    /// Return whether a symbol is one of the ambient builtins seeded into the root symbol table before source
    /// collection.
    pub(in crate::frontend::typechecker) fn is_implicit_builtin_symbol(sym: &Symbol) -> bool {
        sym.scope == 0 && sym.span == Span::default()
    }

    /// Return `true` when an implicit builtin-call root is shadowed by a real source/import binding.
    ///
    /// Decorated functions are intentionally rebound from `Function` symbols to callable `Variable` symbols after
    /// decorator checking. Builtin dispatch therefore has to ask whether the name is still the ambient builtin binding,
    /// not whether the symbol is specifically a `Function`.
    pub(in crate::frontend::typechecker) fn has_non_builtin_call_root_binding(&self, name: &str) -> bool {
        self.lookup_symbol(name)
            .is_some_and(|sym| !Self::is_implicit_builtin_symbol(sym))
    }

    /// Return the active stdlib surface helper imported under `name`, if the import has not been shadowed.
    pub(in crate::frontend::typechecker) fn active_surface_function_import(&self, name: &str) -> Option<SurfaceFnId> {
        let (id, imported_symbol_id) = self.surface_function_import_bindings.get(name)?;
        (self.symbols.lookup(name) == Some(*imported_symbol_id)).then_some(*id)
    }

    /// Return the active stdlib surface type imported under `name`, if the import has not been shadowed.
    pub(in crate::frontend::typechecker) fn active_surface_type_import(&self, name: &str) -> Option<SurfaceTypeId> {
        let (id, imported_symbol_id) = self.surface_type_import_bindings.get(name)?;
        (self.symbols.lookup(name) == Some(*imported_symbol_id)).then_some(*id)
    }
}
