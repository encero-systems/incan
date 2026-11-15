//! Materialize explicit Owned local initialization and retain its exact conditional cleanup relationship.

use super::{BodyBuilder, ast, bir};
use crate::typechecker::CheckedInputSource;
use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

impl BodyBuilder<'_, '_> {
    /// An Owned destination receives the checked initializer's actual value. Only a whole stored non-Copy transfer
    /// retains conditional cleanup metadata; fresh temporaries and Copy inputs cannot supply a source-owner fact.
    pub(super) fn lower_owned_input_binding(
        &mut self,
        assignment: &ast::AssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        identity: &CanonicalSymbolId,
    ) -> Result<(), String> {
        let proof = self
            .input_body
            .as_ref()
            .map_err(|reason| reason.to_string())?
            .as_ref()
            .ok_or("owned local has no owning source body proof")?;
        let initializer = proof.owned_local(identity, assignment.value.span)?;
        let ty = self.checked_type(initializer.value_type);
        let source = initializer.source;
        let source_identity = match &source {
            Some(CheckedInputSource::Place { root, fields, elements }) if fields.is_empty() && elements.is_empty() => {
                Some(root.clone())
            }
            _ => None,
        };
        if self.resolve_ty(assignment.value.span) != ty {
            return Err("owned initializer cannot coerce or copy its source".into());
        }
        if self.identity_bindings.contains_key(identity) || self.input_aliases.contains_key(identity) {
            return Err("owned destination already has storage or a loan".into());
        }
        let value = match source {
            Some(CheckedInputSource::Place { root, fields, elements }) => {
                let place = self.lower_expr_to_place(&assignment.value, scope, out);
                self.authenticate_input_place(&place, &root, &fields, &elements)?;
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
            Some(CheckedInputSource::CarriedLocal) => {
                return Err("owned local source must not retain a carrier local loan".into());
            }
            Some(CheckedInputSource::Temporary) => {
                return Err("owned local source must not retain an invocation temporary".into());
            }
        };
        let place = self
            .assignment_target_place(assignment, ty, remaining, scope, span, out)
            .ok_or("owned destination has no checked storage")?;
        let transferred = matches!(&value, bir::Operand::Place(read)
    if read.fact == bir::OwnershipFact::Move && read.place.projection.is_empty());
        let initializer = if transferred && source_identity.is_some() {
            if !place.projection.is_empty()
                || place
                    .local_id()
                    .and_then(|id| self.locals.get(id.index()))
                    .and_then(|local| local.identity.as_deref())
                    != Some(identity)
            {
                return Err("owned initializer destination differs from its checked declaration".into());
            }
            source_identity.map(|source| bir::CheckedOwnedInitializer {
                source,
                destination: identity.clone(),
                assignment: span,
            })
        } else {
            None
        };
        self.finish_assignment(assignment, place, value, false, span, out);
        if let Some(initializer) = initializer {
            if !matches!(out.last(), Some(bir::Statement { kind: bir::StatementKind::Assign { .. }, span: emitted }) if *emitted == span)
            {
                return Err("owned initializer did not publish its checked assignment".into());
            }
            self.input_owned_initializers.push(initializer);
        }
        Ok(())
    }
}
