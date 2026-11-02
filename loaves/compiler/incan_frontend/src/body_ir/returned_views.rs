//! Returned views: a view body returns a borrow of its declared source place, and a caller aliases the
//! substituted place of a view call rather than holding the returned reference.

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

use super::{BodyBuilder, ast, bir};
use crate::ownership_metadata::SourceSlot;

/// The declared view of the body being lowered: the borrowed parameter local and its stored-field path.
#[derive(Debug, Clone)]
pub(super) struct ReturnedViewSource {
    pub view: bir::ReturnedView,
    pub local: bir::LocalId,
    pub fields: Vec<CanonicalSymbolId>,
}

impl BodyBuilder<'_, '_> {
    /// Select the owner's declared view once its parameter locals exist. A method's receiver is slot 0.
    pub(super) fn prepare_returned_view(
        &mut self,
        owner: Option<&CanonicalSymbolId>,
        params: &[bir::CallableParam],
        has_receiver: bool,
    ) {
        let contract = owner.and_then(|owner| self.type_info.declarations.ownership_contracts.get(owner));
        let Ok(Some(view)) = crate::ownership_metadata::returned_view(contract) else {
            return;
        };
        let slot = match view.source.slot {
            SourceSlot::Receiver if has_receiver => 0,
            SourceSlot::Receiver => return,
            SourceSlot::Parameter(slot) => slot + usize::from(has_receiver),
        };
        let Some(parameter) = params.get(slot) else { return };
        self.returned_view = Some(ReturnedViewSource {
            view: bir::ReturnedView {
                slot,
                access: view.access,
            },
            local: parameter.local,
            fields: view.source.fields.clone(),
        });
    }

    /// Return a borrow of exactly the declared source place. The checker proved the relation; this authenticates
    /// the lowered place against the declared local and field identities before borrowing it.
    pub(super) fn lower_view_return(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(), String> {
        let source = self.returned_view.clone().ok_or("view return has no declared source")?;
        let place = self.lower_expr_to_place(value, scope, out);
        let authenticated = place.local_id() == Some(source.local)
            && place.projection.len() == source.fields.len()
            && place.projection.iter().zip(&source.fields).all(|(projection, field)| {
                matches!(projection, bir::PlaceElem::Field { canonical: Some(identity), structural: false, .. }
                    if identity == field)
            });
        if !authenticated {
            return Err("returned view differs from its declared source place".into());
        }
        let fact = view_fact(source.view.access);
        out.push(bir::Statement {
            kind: bir::StatementKind::Return {
                value: Some(bir::Operand::place(place, fact, false)),
            },
            span,
        });
        Ok(())
    }

    /// For a call whose selected callee returns a view, the substituted place: the receiver's or the slot
    /// argument's place followed by the declared fields, with the view's access. Other calls return `None`.
    pub(super) fn view_call_place(
        &self,
        span: ast::Span,
        receiver: Option<&bir::Operand>,
        arguments: &[bir::ArgumentElement],
    ) -> Option<Result<(bir::Place, IncanInputAccess), String>> {
        let (access, slot, fields) = self.type_info.selected_call_view(span)?;
        Some((|| {
            let base = match slot {
                SourceSlot::Receiver => receiver,
                SourceSlot::Parameter(slot) => match arguments.get(slot) {
                    Some(bir::ArgumentElement::One(operand)) => Some(operand),
                    _ => None,
                },
            };
            let Some(bir::Operand::Place(read)) = base else {
                return Err("returned view source is not borrowed from existing storage".to_string());
            };
            let mut place = read.place.clone();
            place.projection.extend(
                fields
                    .into_iter()
                    .map(|field| bir::PlaceElem::field(field.declaration_name.clone(), Some(field))),
            );
            Ok((place, access))
        })())
    }

    /// Emit a view call for its effects only. The returned reference is not retained: the caller uses the
    /// substituted place, so no reference-valued local, wrapper or copy exists.
    pub(super) fn push_view_call(
        &mut self,
        callee: bir::Callee,
        args: Vec<bir::ArgumentElement>,
        place: bir::Place,
        access: IncanInputAccess,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: None,
                callee,
                args,
                may_panic: false,
            },
            span,
        });
        bir::Operand::place(place, view_fact(access), false)
    }
}

/// The ownership fact of a borrow with the view's access.
fn view_fact(access: IncanInputAccess) -> bir::OwnershipFact {
    if access == IncanInputAccess::Exclusive {
        bir::OwnershipFact::MutBorrow
    } else {
        bir::OwnershipFact::Borrow
    }
}
