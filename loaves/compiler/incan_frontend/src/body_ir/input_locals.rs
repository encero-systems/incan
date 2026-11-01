//! Explicit local loans project to existing source storage rather than copied or independently dropped locals.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::CanonicalSymbolId;

use super::{BodyBuilder, ast, bir};
use crate::typechecker::CheckedInputSource;

/// Physical place selected by a checked canonical alias. The source graph, not this map, owns lifetime analysis.
pub(super) struct LocalInputAlias {
    /// Existing owner and static field projection; no separate reference-valued frame slot is introduced.
    pub place: bir::Place,
    /// Binding-level write permission remains separate from having exclusive access.
    pub writable: bool,
}

impl BodyBuilder<'_, '_> {
    /// Nested evaluators without this body's proof cannot reuse its source aliases, even on a reused builder.
    pub(super) fn input_alias(&self, identity: &CanonicalSymbolId) -> Option<&LocalInputAlias> {
        self.input_body.as_ref().ok()?.as_ref()?;
        self.input_aliases.get(identity)
    }

    /// Materialize only checked local borrow declarations with existing static Copy referents. Non-Copy alias
    /// value consumers and owned local transfers retain their refusal until their own no-repair projection exists.
    pub(super) fn lower_local_input_binding(
        &mut self,
        assignment: &ast::AssignmentStmt,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(), String>> {
        let identity = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)?
            .clone();
        if identity.declaration_span != assignment.name_span {
            return None;
        }
        if !self.type_info.has_local_input_annotation(&identity) {
            return None;
        }
        Some((|| {
            let proof = self
                .input_body
                .as_ref()
                .map_err(|reason| reason.to_string())?
                .as_ref()
                .ok_or("explicit local alias has no owning source body proof")?;
            let mode = proof
                .local_mode(&identity)
                .ok_or("explicit local alias has no checked annotation")?;
            if !matches!(mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) {
                return Err("explicit owned local requires checked transfer materialization".into());
            }
            let loan = proof.local_loan(&identity, assignment.value.span)?;
            let ty = self.checked_type(loan.value_type);
            if !ty.abi_v0_facts().ownership.is_trivially_copy() {
                return Err("non-Copy local alias requires checked value-consumption projection".into());
            }
            if self.resolve_ty(assignment.value.span) != ty {
                return Err("explicit local loan cannot coerce or copy its source".into());
            }
            let CheckedInputSource::Place { root, fields } = loan.source else {
                return Err("explicit local loan requires existing owner storage".into());
            };
            let destination = loan.destination.clone();
            let writable = loan.writable;
            if writable && loan.access != incan_semantics_core::IncanInputAccess::Exclusive {
                return Err("shared local loan cannot acquire write permission".into());
            }
            let place = self.lower_expr_to_place(&assignment.value, scope, out);
            self.authenticate_input_place(&place, &root, &fields)?;
            if self.identity_bindings.contains_key(&destination) || self.input_aliases.contains_key(&destination) {
                return Err("explicit local alias already has an owner or another source place".into());
            }
            self.input_aliases
                .insert(destination, LocalInputAlias { place, writable });
            Ok(())
        })())
    }
}
