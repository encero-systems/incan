//! Canonical storage facts for direct static bindings. Whole-binding assignment detaches a handle; projections
//! continue to name the static. Control-flow-dependent handle selection remains explicit refusal until Body IR
//! can carry the selected handle as a value.

use super::{BodyBuilder, ast, bir, hir_span};
use crate::typechecker::IdentKind;
use incan_semantics_core::SemanticSourceTargetKind;

impl BodyBuilder<'_, '_> {
    /// Retain the exact checked global behind a new direct storage binding or a reassigned existing handle.
    ///
    /// Only a bare source static with the binding's own type creates a live alias, matching legacy. Reading another
    /// local alias, a function result, or a static through a conversion produces an ordinary value instead.
    pub(super) fn record_static_alias_assignment(
        &mut self,
        assignment: &ast::AssignmentStmt,
        destination: &bir::Place,
        value: &bir::Operand,
    ) {
        let Some(local) = destination.local_id() else {
            return;
        };
        let declaration = &self.locals[local.index()];
        let is_declaration = declaration.span == hir_span(assignment.name_span);
        let global = match value {
            bir::Operand::Place(read) if read.place.projection.is_empty() => read.place.global(),
            _ => None,
        };
        let alias = global.filter(|global| {
            matches!(assignment.value.node, ast::Expr::Ident(_))
                && self.type_info.ident_kind(assignment.value.span) == Some(IdentKind::Static)
                && self.type_info.resolved_identity(assignment.value.span) == Some(&global.identity)
                && global.identity.kind == SemanticSourceTargetKind::Static
                && declaration.ty == global.ty
        });
        if is_declaration && alias.is_some() {
            self.static_binding_locals.insert(local);
        }
        if !self.static_binding_locals.contains(&local) {
            return;
        }
        if let Some(global) = alias {
            self.static_aliases.insert(local, global.clone());
        } else {
            self.static_aliases.remove(&local);
        }
    }

    /// Read a storage binding's live value before an operation detaches its whole local binding.
    pub(super) fn static_alias_read_place(&self, place: &bir::Place) -> bir::Place {
        place
            .local_id()
            .and_then(|local| self.static_aliases.get(&local))
            .map_or_else(|| place.clone(), |global| bir::Place::from_global(global.clone()))
    }
}
