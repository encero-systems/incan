//! Closures bound to a local of an explicit body. A closure reads captured Copy values when it is built and borrows
//! every other captured place until its last call. A captured alias or returned view is reborrowed through its own
//! loan, and an exclusive input may be reborrowed shared when the closure only reads it. Passing a closure to a
//! parameter whose body only calls it lends all retained captures for that call; returning or storing it is refused.

use super::super::flow::{ClosureCaptures, InputEvaluation, InvocationArgument};
use super::{Access, Builder, Event, Loan, LoanId, LoanKind, Path, Unavailable};
use crate::ast::{CallArg, Expr, Span, Spanned};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess};

impl Builder<'_> {
    /// Bind a closure to a new local: read each captured Copy value, then borrow every other captured place for as
    /// long as the local can still be called.
    pub(super) fn bind_closure(
        &mut self,
        local: CanonicalSymbolId,
        closure: &Spanned<Expr>,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let refuse = |reason: &str| Unavailable {
            span: closure.span,
            reason: reason.into(),
        };
        let captures = self
            .checker
            .closure_captures(closure.span)
            .ok_or_else(|| refuse("closure has no checked captures"))?
            .to_vec();
        let mut identities = Vec::new();
        let mut borrowed = Vec::new();
        let mut loans = Vec::new();
        let mut reads = Vec::new();
        for capture in &captures {
            let identity = capture
                .identity
                .clone()
                .ok_or_else(|| refuse("closure capture has no checked declaration"))?;
            if self.closures.contains_key(&identity) {
                return Err(refuse("a closure capturing another closure"));
            }
            if self.carriers.contains_key(&identity) || self.taken_carriers.contains(&identity) {
                return Err(refuse("a closure capturing a carried result"));
            }
            if let Some(dependencies) = self.graph.dependent_locals.get(&identity).cloned() {
                for dependency in dependencies {
                    let (next, path, loan) = self.carry_dependency(dependency, closure.span, incoming)?;
                    incoming = next;
                    borrowed.push(path);
                    loans.push((identity.clone(), loan));
                }
            }
            let path = match self.aliases.get(&identity) {
                // A borrowed local or returned view is captured as a reborrow of its own loan.
                Some(path) => path.clone(),
                None => Path {
                    place: self.root(&identity, closure.span)?,
                    through: None,
                },
            };
            let retained_place = path.through.is_some() || self.borrowed_inputs.contains(&path.place.root);
            if self.copy_value(&capture.ty) && !retained_place {
                let read = Event::Access {
                    place: path.place.clone(),
                    access: Access::Read,
                    through: path.through,
                };
                incoming = self.event(incoming, read, closure.span);
                let Some(read) = incoming.last().copied() else {
                    return Err(refuse("closure capture read has no flow node"));
                };
                reads.push((identity.clone(), read));
            } else {
                let loan = LoanId(self.graph.loans.len());
                self.graph.loans.push(Loan {
                    source: path.place.clone(),
                    kind: LoanKind::Shared,
                    writable: false,
                    parent: path.through,
                    span: closure.span,
                });
                incoming = self.event(incoming, Event::Borrow(loan), closure.span);
                loans.push((identity.clone(), loan));
                borrowed.push(Path {
                    place: path.place,
                    through: Some(loan),
                });
            }
            identities.push(identity);
        }
        let closure_record = ClosureCaptures {
            local: local.clone(),
            captures: identities,
            loans,
            reads,
        };
        self.graph.closures.insert(closure.span, closure_record);
        self.closures.insert(local, borrowed);
        Ok(incoming)
    }

    /// The closure local an expression names, if it names one: a callee, or an argument passed on.
    pub(super) fn closure_local(&self, expression: &Spanned<Expr>) -> Option<CanonicalSymbolId> {
        if !matches!(expression.node, Expr::Ident(_)) {
            return None;
        }
        let identity = self.checker.type_info.resolved_identity(expression.span)?;
        self.closures.contains_key(identity).then(|| identity.clone())
    }

    /// Call a closure local: evaluate its Copy arguments, then use the borrow it holds.
    pub(super) fn closure_call(
        &mut self,
        closure: &CanonicalSymbolId,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for argument in arguments {
            let CallArg::Positional(value) = argument else {
                return Err(Unavailable {
                    span,
                    reason: "a closure call needs positional arguments".into(),
                });
            };
            let copy = self
                .checker
                .type_info
                .expr_type(value.span)
                .is_some_and(|ty| self.copy_value(ty));
            if !copy {
                return Err(Unavailable {
                    span: value.span,
                    reason: "a closure call argument needs a Copy value".into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        let borrowed = self.closures.get(closure).cloned().unwrap_or_default();
        for path in borrowed {
            incoming = {
                let use_of_borrow = Event::Access {
                    place: path.place,
                    access: Access::Read,
                    through: path.through,
                };
                self.event(incoming, use_of_borrow, span)
            };
        }
        Ok(incoming)
    }

    /// Pass a closure local to a parameter its callee only calls: use the borrow it holds, then read the closure local
    /// itself, which the callee borrows for the call. A parameter that may keep the closure is refused.
    pub(super) fn closure_argument(
        &mut self,
        closure: &CanonicalSymbolId,
        callee: &CanonicalSymbolId,
        expression: &Spanned<Expr>,
        position: (usize, usize),
        mut incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, InvocationArgument), Unavailable> {
        let (slot, written_position) = position;
        if !self.checker.closure_parameter_only_called(callee, slot) {
            return Err(Unavailable {
                span: expression.span,
                reason: "a closure passed to a parameter that may keep it".into(),
            });
        }
        let start_node = self.graph.nodes.len();
        incoming = self.event(incoming, Event::Nothing, expression.span);
        let borrowed = self.closures.get(closure).cloned().unwrap_or_default();
        for path in borrowed {
            let use_of_borrow = Event::Access {
                place: path.place,
                access: Access::Read,
                through: path.through,
            };
            incoming = self.event(incoming, use_of_borrow, expression.span);
        }
        let place = self.root(closure, expression.span)?;
        let source_node = self.graph.nodes.len();
        let read = Event::Access {
            place: place.clone(),
            access: Access::Read,
            through: None,
        };
        incoming = self.event(incoming, read, expression.span);
        let end_node = self.graph.nodes.len();
        incoming = self.event(incoming, Event::Nothing, expression.span);
        let argument = InvocationArgument {
            span: expression.span,
            slot,
            written_position,
            access: IncanInputAccess::Inferred,
            loan: None,
            source: Some(place),
            evaluation: InputEvaluation {
                start_node,
                end_node,
                source_node: Some(source_node),
            },
        };
        Ok((incoming, argument))
    }
}
