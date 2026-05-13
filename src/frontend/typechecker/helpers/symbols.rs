//! Shared symbol-resolution predicates used across typechecker phases.
//!
//! These helpers keep implicit-root-builtin detection in one place so import collection and
//! expression checking make the same shadowing decision.

use crate::frontend::ast::Span;
use crate::frontend::symbols::SymbolKind;
use crate::frontend::typechecker::TypeChecker;

impl TypeChecker {
    /// Return `true` when `name` resolves to a non-builtin function definition.
    ///
    /// Call checking uses this to decide whether builtin dispatch should yield to a user/imported function of the same
    /// name.
    pub(in crate::frontend::typechecker) fn has_non_builtin_function_definition(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|sym| {
            matches!(sym.kind, SymbolKind::Function(_)) && !(sym.scope == 0 && sym.span == Span::default())
        })
    }
}
