//! Materialize explicit direct-call inputs only from their checked invocation and existing source storage.

use super::args::{DeclaredSlot, plan_declared_args};
use super::{BodyBuilder, IncanType, ast, bir, hir_span};
use crate::typechecker::{CheckedInputInvocation, CheckedInputSource};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess};

#[cfg(test)]
mod tests;

impl BodyBuilder<'_, '_> {
    /// Keep ordinary argument lowering unchanged. An explicit obligation with missing owner, target or flow proof
    /// refuses rather than acquiring inferred passing semantics through a default or indirect evaluation frame.
    pub(super) fn bind_explicit_input_args(
        &mut self,
        target: Option<&CanonicalSymbolId>,
        arguments: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String>> {
        if !self.type_info.has_explicit_input_invocation(span, target) {
            return None;
        }
        Some((|| {
            let target = target.ok_or("explicit input call has no canonical target")?;
            let proof = self
                .input_body
                .as_ref()
                .map_err(|reason| reason.to_string())?
                .as_ref()
                .ok_or("explicit input call has no owning source body proof")?;
            let invocation = proof.invocation(span, target)?;
            if invocation.receiver.is_some() {
                return Err("explicit method inputs require their checked receiver materialization".into());
            }
            self.lower_explicit_input_args(invocation, arguments, scope, out)
        })())
    }

    /// Borrow a checked method receiver in its existing owner storage, then bind the explicit arguments in the
    /// graph's authenticated order. The receiver is evaluated before the arguments, as the source graph records.
    pub(super) fn bind_explicit_method_input(
        &mut self,
        target: Option<&CanonicalSymbolId>,
        receiver: &ast::Spanned<ast::Expr>,
        arguments: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Result<(bir::Operand, Vec<bir::ArgumentElement>, bir::ArgumentBinding), String>> {
        if !self.type_info.has_explicit_input_invocation(span, target) {
            return None;
        }
        Some((|| {
            let target = target.ok_or("explicit method call has no canonical target")?;
            let proof = self
                .input_body
                .as_ref()
                .map_err(|reason| reason.to_string())?
                .as_ref()
                .ok_or("explicit method call has no owning source body proof")?;
            let mut invocation = proof.invocation(span, target)?;
            let checked = invocation
                .receiver
                .take()
                .ok_or("explicit method call has no checked receiver")?;
            let access = if checked.owned {
                IncanInputAccess::Owned
            } else if checked.exclusive {
                IncanInputAccess::Exclusive
            } else {
                IncanInputAccess::Shared
            };
            let receiver = match (checked.source, checked.owned) {
                (Some(CheckedInputSource::Place { root, fields, elements }), false) => {
                    let place = self.lower_expr_to_place(receiver, scope, out);
                    self.authenticate_input_place(&place, &root, &fields, &elements)?;
                    self.consume_input_source_read(receiver, &place);
                    let fact = if checked.exclusive {
                        bir::OwnershipFact::MutBorrow
                    } else {
                        bir::OwnershipFact::Borrow
                    };
                    bir::Operand::place(place, fact, false)
                }
                (Some(CheckedInputSource::Place { root, fields, elements }), true) => {
                    let ty = self
                        .type_info
                        .expr_type(receiver.span)
                        .map(|ty| self.checked_type(ty))
                        .ok_or("explicit method receiver has no checked type")?;
                    let place = self.lower_expr_to_place(receiver, scope, out);
                    self.authenticate_input_place(&place, &root, &fields, &elements)?;
                    self.explicit_place_operand(place, &ty, access, receiver)?
                }
                (Some(CheckedInputSource::Temporary) | None, _) => {
                    let ty = self
                        .type_info
                        .expr_type(receiver.span)
                        .map(|ty| self.checked_type(ty))
                        .ok_or("explicit method receiver has no checked type")?;
                    let value = self.lower_boundary_value(receiver, Some(&ty), scope, out);
                    self.explicit_temporary_operand(value, ty, access, receiver.span, scope, out)?
                }
                (Some(CheckedInputSource::CarriedLocal), _) => {
                    return Err("explicit method receiver cannot use a retained carrier local proof".into());
                }
            };
            let (operands, binding) = self.lower_explicit_input_args(invocation, arguments, scope, out)?;
            Ok((receiver, operands, binding))
        })())
    }

    /// Evaluate in the graph's authenticated order and retain written positions for the existing binding protocol.
    /// Closed call-site value types are used here; declaration wrappers are never treated as runtime containers.
    fn lower_explicit_input_args(
        &mut self,
        invocation: CheckedInputInvocation<'_>,
        arguments: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let slots = invocation
            .parameters
            .iter()
            .map(DeclaredSlot::from_checked_param)
            .collect::<Vec<_>>();
        let planned = plan_declared_args("explicit input call", &slots, arguments)?;
        if planned.len() != invocation.arguments.len() {
            return Err("explicit input call does not retain every supplied argument".into());
        }
        let declaration_order = invocation.declaration_order;
        let argument_count = invocation.arguments.len();
        let mut lowered = (0..slots.len()).map(|_| None).collect::<Vec<_>>();
        for argument in invocation.arguments {
            let (slot, expression) = planned
                .get(argument.written_position)
                .ok_or("explicit input lost its written argument")?;
            if *slot != argument.slot || expression.span != argument.span {
                return Err("explicit input argument differs from its checked source binding".into());
            }
            let parameter = invocation
                .parameters
                .get(*slot)
                .ok_or("explicit input has no selected value parameter")?;
            let ty = super::parameter_types::physical_type_argument(self.checked_type(&parameter.ty));
            let value = match argument.source {
                Some(CheckedInputSource::Place { root, fields, elements }) => {
                    if super::parameter_types::physical_type_argument(self.resolve_ty(expression.span)) != ty {
                        return Err("explicit source-place input cannot coerce or copy its owner".into());
                    }
                    let place = self.lower_expr_to_place(expression, scope, out);
                    self.authenticate_input_place(&place, &root, &fields, &elements)?;
                    self.explicit_place_operand(place, &ty, argument.access, expression)?
                }
                Some(CheckedInputSource::CarriedLocal) => {
                    return Err("explicit input argument cannot use a retained carrier local proof".into());
                }
                Some(CheckedInputSource::Temporary) | None => {
                    let value = self.lower_boundary_value(expression, Some(&ty), scope, out);
                    self.explicit_temporary_operand(value, ty, argument.access, expression.span, scope, out)?
                }
            };
            let entry = lowered
                .get_mut(*slot)
                .ok_or("explicit input parameter slot is missing")?;
            if entry.replace((value, argument.written_position)).is_some() {
                return Err("explicit input parameter was materialized twice".into());
            }
        }
        let mut operands = Vec::with_capacity(argument_count);
        let mut bindings = Vec::with_capacity(argument_count);
        let mut defaulted_slots = Vec::new();
        for (slot, argument) in lowered.into_iter().enumerate() {
            match argument {
                Some((operand, written_position)) => {
                    operands.push(bir::ArgumentElement::One(operand));
                    bindings.push(bir::BoundArgument { slot, written_position });
                }
                None if invocation
                    .parameters
                    .get(slot)
                    .is_some_and(|parameter| parameter.has_default) =>
                {
                    defaulted_slots.push(slot);
                }
                None => return Err("explicit input call omits a required selected parameter".into()),
            }
        }
        let binding = if declaration_order {
            bir::ArgumentBinding::ResolvedDeclarationOrder {
                arguments: bindings,
                defaulted_slots,
            }
        } else {
            bir::ArgumentBinding::Resolved {
                arguments: bindings,
                defaulted_slots,
            }
        };
        Ok((operands, binding))
    }

    /// The physical local and every stored field must name the checked source place; similar names or layouts do
    /// not authorize a borrow. Indexed, global and adapted carriers remain refused by this first consumer.
    pub(super) fn authenticate_input_place(
        &self,
        place: &bir::Place,
        root: &CanonicalSymbolId,
        fields: &[CanonicalSymbolId],
        elements: &[usize],
    ) -> Result<(), String> {
        let local = place
            .local_id()
            .and_then(|id| self.locals.get(id.index()))
            .ok_or("explicit input has no local owner storage")?;
        if local.identity.as_ref() != Some(root) || place.projection.len() != fields.len() + elements.len() {
            return Err("explicit input storage differs from its checked source owner".into());
        }
        let mut fields = fields.iter();
        for (position, projection) in place.projection.iter().enumerate() {
            if elements.contains(&position) {
                if !matches!(projection, bir::PlaceElem::Index(_)) {
                    return Err("explicit input storage differs from its checked element projection".into());
                }
            } else {
                let field = fields
                    .next()
                    .ok_or("explicit input lost its checked field projection")?;
                if !matches!(projection, bir::PlaceElem::Field { canonical: Some(identity), structural: false, .. } if identity == field)
                {
                    return Err("explicit input storage differs from its checked field projection".into());
                }
            }
        }
        if fields.next().is_some() {
            return Err("explicit input did not materialize every checked field projection".into());
        }
        Ok(())
    }

    /// Borrow in place or transfer the actual owner. Conditional whole transfers require authenticated subjects
    /// and statement-if or statement-loop state joins; other conditional or partial transfers remain refused without
    /// repair clones.
    pub(super) fn explicit_place_operand(
        &mut self,
        place: bir::Place,
        ty: &IncanType,
        access: IncanInputAccess,
        expression: &ast::Spanned<ast::Expr>,
    ) -> Result<bir::Operand, String> {
        let local = place.local_id().ok_or("explicit source input has no local storage")?;
        let proof = self
            .input_body
            .as_ref()
            .map_err(|reason| reason.to_string())?
            .as_ref()
            .ok_or("explicit input has no owning source body proof")?;
        let copy = proof.value_is_copy(expression.span)?;
        // A closure local passed to a parameter its callee only calls lends the callee its callable object.
        let passed_closure = proof.is_closure_argument(expression.span);
        if matches!(access, IncanInputAccess::Owned | IncanInputAccess::Inferred)
            && copy != ty.abi_v0_facts().ownership.is_trivially_copy()
        {
            return Err("checked Copy semantics require a compatible native input carrier".into());
        }
        let (fact, last_use) = match access {
            IncanInputAccess::Shared | IncanInputAccess::InferredShared => (bir::OwnershipFact::Borrow, false),
            IncanInputAccess::Exclusive => (bir::OwnershipFact::MutBorrow, false),
            IncanInputAccess::Owned if copy => (bir::OwnershipFact::Copy, false),
            IncanInputAccess::Owned => {
                if !place.projection.is_empty() || !self.input_transfer_path_supported(local) {
                    return Err("explicit projected or conditional transfer requires checked drop obligations".into());
                }
                if self.is_borrowed_parameter(local) || self.is_receiver_local(local) {
                    return Err("explicit transfer cannot consume an inferred borrowed parameter or receiver".into());
                }
                self.moved_out.insert(local);
                (bir::OwnershipFact::Move, true)
            }
            IncanInputAccess::Inferred if copy => (bir::OwnershipFact::Copy, false),
            IncanInputAccess::Inferred if passed_closure => (bir::OwnershipFact::Borrow, false),
            IncanInputAccess::Inferred => {
                return Err("ordinary non-Copy input requires its inferred access proof".into());
            }
        };
        self.consume_input_source_read(expression, &place);
        Ok(bir::Operand::place(place, fact, last_use))
    }

    /// A fresh value may own temporary storage for the invocation. Existing places must use the authenticated
    /// source route; a Clone operand never becomes an explicit ownership transfer by changing its tag.
    pub(super) fn explicit_temporary_operand(
        &mut self,
        mut value: bir::Operand,
        ty: IncanType,
        access: IncanInputAccess,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<bir::Operand, String> {
        if let bir::Operand::Place(read) = &value {
            let local = read.place.local_id().and_then(|id| self.locals.get(id.index()));
            if !read.place.projection.is_empty()
                || local.is_none_or(|local| local.origin != bir::LocalOrigin::Temporary)
                || !matches!(read.fact, bir::OwnershipFact::Copy | bir::OwnershipFact::Move)
            {
                return Err("explicit temporary input requires a fresh owned result".into());
            }
        }
        if matches!(
            access,
            IncanInputAccess::Shared | IncanInputAccess::InferredShared | IncanInputAccess::Exclusive
        ) {
            if !matches!(value, bir::Operand::Place(_)) {
                value = self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(span), out);
            }
            let bir::Operand::Place(read) = &mut value else {
                return Err("explicit temporary could not be materialized".into());
            };
            self.retain_argument_borrow(read, access == IncanInputAccess::Exclusive);
        }
        Ok(value)
    }
}
