//! Explicit local declarations project checked loans to existing storage and owned initializers to new owners.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{BodyBuilder, ast, bir};
use crate::typechecker::CheckedInputSource;

mod reads;

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

    /// Materialize checked aliases in existing storage and owned initializers as independent destinations. The
    /// source graph authenticates each relationship; ordinary assignment never supplies a fallback repair clone.
    pub(super) fn lower_local_input_binding(
        &mut self,
        assignment: &ast::AssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
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
            if mode == OwnershipMode::Owned {
                let initializer = proof.owned_local(&identity, assignment.value.span)?;
                let ty = self.checked_type(initializer.value_type);
                let source = initializer.source;
                if self.resolve_ty(assignment.value.span) != ty {
                    return Err("owned initializer cannot coerce or copy its source".into());
                }
                if self.identity_bindings.contains_key(&identity) || self.input_aliases.contains_key(&identity) {
                    return Err("owned destination already has storage or a loan".into());
                }
                let value = match source {
                    Some(CheckedInputSource::Place { root, fields }) => {
                        let place = self.lower_expr_to_place(&assignment.value, scope, out);
                        self.authenticate_input_place(&place, &root, &fields)?;
                        self.explicit_place_operand(place, &ty, IncanInputAccess::Owned, &assignment.value)?
                    }
                    None => {
                        let value = self.lower_boundary_value(&assignment.value, Some(&ty), scope, out);
                        self.explicit_temporary_operand(
                            value,
                            ty.clone(),
                            IncanInputAccess::Owned,
                            assignment.value.span,
                            scope,
                            out,
                        )?
                    }
                    Some(CheckedInputSource::Temporary) => {
                        return Err("owned local source must not retain an invocation temporary".into());
                    }
                };
                let place = self
                    .assignment_target_place(assignment, ty, remaining, scope, span, out)
                    .ok_or("owned destination has no checked storage")?;
                self.finish_assignment(assignment, place, value, false, span, out);
                return Ok(());
            }
            if !matches!(mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) {
                return Err("explicit returned local requires checked dependency materialization".into());
            }
            let loan = proof.local_loan(&identity, assignment.value.span)?;
            let ty = self.checked_type(loan.value_type);
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
            self.consume_input_source_read(&assignment.value, &place);
            if self.identity_bindings.contains_key(&destination) || self.input_aliases.contains_key(&destination) {
                return Err("explicit local alias already has an owner or another source place".into());
            }
            self.input_aliases
                .insert(destination, LocalInputAlias { place, writable });
            Ok(())
        })())
    }
}
