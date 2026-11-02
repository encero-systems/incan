//! Explicit local declarations project checked loans to existing storage and owned initializers to new owners.

use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{BodyBuilder, ast, bir, hir_span};
use crate::typechecker::CheckedInputSource;

mod owned;
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
            return self.lower_view_local(assignment, &identity, scope, out);
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
                self.retain_input_local_binding(assignment, &identity, IncanInputAccess::Owned);
                return self.lower_owned_input_binding(assignment, remaining, scope, span, out, &identity);
            }
            if !matches!(mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut) {
                return Err("explicit returned local requires checked dependency materialization".into());
            }
            let loan = proof.local_loan(&identity, assignment.value.span)?;
            self.retain_input_local_binding(assignment, &identity, loan.access);
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

    /// A new unannotated local initialized by a returned view aliases the call's substituted place: the call runs
    /// for its effects, and the local names existing storage rather than a reference-valued slot or a copy.
    fn lower_view_local(
        &mut self,
        assignment: &ast::AssignmentStmt,
        identity: &CanonicalSymbolId,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(), String>> {
        let access = self
            .input_body
            .as_ref()
            .ok()?
            .as_ref()?
            .view_local(identity, hir_span(assignment.value.span));
        let access = match access {
            Ok(access) => access?,
            Err(reason) => return Some(Err(reason.into())),
        };
        self.retain_input_local_binding(assignment, identity, access);
        Some((|| {
            let bir::Operand::Place(read) = self.lower_expr_to_operand(&assignment.value, scope, out) else {
                return Err("returned view local has no substituted place".into());
            };
            if self.identity_bindings.contains_key(identity) || self.input_aliases.contains_key(identity) {
                return Err("returned view local already has an owner or another source place".into());
            }
            let writable = access == IncanInputAccess::Exclusive && assignment.binding == ast::BindingKind::Mutable;
            self.input_aliases.insert(
                identity.clone(),
                LocalInputAlias {
                    place: read.place,
                    writable,
                },
            );
            Ok(())
        })())
    }

    /// Record the binding's obligation before lowering it, so a body that later refuses still names every explicit
    /// local it declared.
    fn retain_input_local_binding(
        &mut self,
        assignment: &ast::AssignmentStmt,
        identity: &CanonicalSymbolId,
        access: IncanInputAccess,
    ) {
        self.input_local_bindings.push(bir::ExplicitInputBinding {
            declaration: hir_span(assignment.name_span),
            identity: Some(identity.clone()),
            access,
        });
    }
}
