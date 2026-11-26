//! Two-phase borrows. Rust reserves the implicit reborrow of a `&mut` binding passed as an argument, and the autoref
//! of a `mut self` receiver, while the call's later arguments are evaluated, and activates it at the call. A reserved
//! borrow acts as a shared loan: later arguments may read, clone or compare the place, but a write, a move or another
//! exclusive borrow of it is refused, as rustc refuses them.

use super::super::super::flow::Invocation;
use super::super::{Access, Builder, Event, LoanId, Path, Root, Unavailable};
use crate::ast::{Expr, Span, Spanned};
use crate::symbols::ResolvedType;

/// Where an activated borrow is held: the call's receiver or one of its arguments.
#[derive(Debug, Clone, Copy)]
pub(in crate::typechecker::ownership_contracts::source) enum ReservedSlot {
    Receiver,
    Argument(usize),
}

/// One exclusive borrow of a call held as a shared reservation until the call.
#[derive(Debug, Clone, Copy)]
pub(in crate::typechecker::ownership_contracts::source) struct Reservation {
    pub(in crate::typechecker::ownership_contracts::source) slot: ReservedSlot,
    pub(in crate::typechecker::ownership_contracts::source) loan: LoanId,
    pub(in crate::typechecker::ownership_contracts::source) writable: bool,
    pub(in crate::typechecker::ownership_contracts::source) span: Span,
}

impl Builder<'_> {
    /// Whether an exclusive argument is Rust's implicit reborrow of a `&mut` binding: the whole binding, holding an
    /// exclusive reference or bound to an exclusive borrowed input. An owned place lent exclusively is an explicit
    /// borrow, which Rust does not reserve. Only the loan-conflict pass outside the whole-body proof reserves: a
    /// proof body projects each argument's loan into its evaluation, so it keeps the borrow at the argument.
    pub(in crate::typechecker::ownership_contracts::source) fn reborrowed_exclusive_argument(
        &mut self,
        expression: &Spanned<Expr>,
    ) -> Result<bool, Unavailable> {
        if !self.checker.native_borrow_pass {
            return Ok(false);
        }
        let inner = match &expression.node {
            Expr::Paren(inner) => inner.as_ref(),
            _ => expression,
        };
        if !matches!(inner.node, Expr::Ident(_)) {
            return Ok(false);
        }
        if matches!(
            self.checker.type_info.expr_type(inner.span),
            Some(ResolvedType::RefMut(_))
        ) {
            return Ok(true);
        }
        let Some(path) = self.optional_place(inner)? else {
            return Ok(false);
        };
        Ok(path.place.projection.is_empty()
            && matches!(path.place.root, Root::Binding(_))
            && self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| permission.exclusive && !permission.transferable))
    }

    /// At the call, end each reservation with a read through it and start its exclusive loan on the same place; the
    /// invocation and its retained loans hold the exclusive loan from then on.
    pub(in crate::typechecker::ownership_contracts::source) fn activate_reservations(
        &mut self,
        reservations: &[Reservation],
        invocation: &mut Invocation,
        loans: &mut [LoanId],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for reservation in reservations {
            let reserved = self.graph.loans.get(reservation.loan.0).cloned().ok_or(Unavailable {
                span: reservation.span,
                reason: "reserved borrow has no loan".into(),
            })?;
            incoming = self.event(
                incoming,
                Event::Access {
                    place: reserved.source.clone(),
                    access: Access::Read,
                    through: Some(reservation.loan),
                },
                reservation.span,
            );
            let active = self.call_loan(
                Path {
                    place: reserved.source,
                    through: reserved.parent,
                },
                true,
                reservation.writable,
                reservation.span,
            );
            incoming = self.event(incoming, Event::Borrow(active), reservation.span);
            for loan in loans.iter_mut().filter(|loan| **loan == reservation.loan) {
                *loan = active;
            }
            match reservation.slot {
                ReservedSlot::Receiver => invocation.receiver = Some(active),
                ReservedSlot::Argument(index) => {
                    if let Some(argument) = invocation.arguments.get_mut(index) {
                        argument.loan = Some(active);
                    }
                }
            }
        }
        Ok(incoming)
    }
}
