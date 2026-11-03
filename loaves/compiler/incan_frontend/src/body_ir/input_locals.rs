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
            return self.lower_view_local(assignment, &identity, remaining, scope, span, out);
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
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(), String>> {
        let access = self
            .input_body
            .as_ref()
            .ok()?
            .as_ref()?
            .view_local(identity, &assignment.value);
        let access = match access {
            Ok(access) => access?,
            Err(reason) => return Some(Err(reason.into())),
        };
        self.retain_input_local_binding(assignment, identity, access);
        // A local bound to a call that carries a view stores the carrier itself, typed as the call's own result; a
        // later match takes the view out of it at the call's substituted place.
        if let Some((call, payload)) = self.carried_view_call(&assignment.value) {
            self.carried_view_locals.insert(identity.clone(), call);
            return Some((|| {
                let ty = super::returned_views::reference_carrier(self.resolve_ty(call), access, payload);
                let value = self.lower_expr_to_operand(&assignment.value, scope, out);
                let place = self
                    .assignment_target_place(assignment, ty, remaining, scope, span, out)
                    .ok_or("carried view local has no checked storage")?;
                self.finish_assignment(assignment, place, value, false, span, out);
                Ok(())
            })());
        }
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

    /// Bind the arm of the variant holding the view (`Ok` or `Some`, or `Err` for a view carried in the error) of a
    /// match on a carried view's call to the call's substituted place, as `view = call()?` binds it. A pattern
    /// binding follows the place it binds, so an exclusive view writes through it. Returns the identity to unbind
    /// after the arm, or `None` when the arm binds an ordinary payload.
    pub(super) fn bind_carried_arm_view(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        view: &(bir::Place, IncanInputAccess, usize),
    ) -> Option<CanonicalSymbolId> {
        let (place, access, payload) = view.clone();
        let (name, span) = crate::typechecker::carried_view_binding(pattern, payload)?;
        let identity = self.type_info.resolved_write_identity(span, name)?.clone();
        self.input_local_bindings.push(bir::ExplicitInputBinding {
            declaration: hir_span(span),
            identity: Some(identity.clone()),
            access,
        });
        let writable = access == IncanInputAccess::Exclusive;
        self.input_aliases
            .insert(identity.clone(), LocalInputAlias { place, writable });
        Some(identity)
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
