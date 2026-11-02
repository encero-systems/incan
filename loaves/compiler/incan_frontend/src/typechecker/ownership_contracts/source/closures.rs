//! Closures bound to a local of an explicit body. A closure reads the locals it captures when it is built, so a
//! captured Copy value is read there and a captured borrow becomes a shared loan the local holds; every call of the
//! local is a use of that borrow; a captured `Borrowed[T]` local is reborrowed through its own loan. The native closure
//! holds its captures by value, so a captured borrow must be of a Copy value. A closure that escapes, reads the
//! receiver, or captures a value that is not Copy, a view a call returned, an exclusive borrow or more than one borrow
//! is refused.

use super::super::flow::ClosureCaptures;
use super::{Access, Builder, Event, Loan, LoanId, LoanKind, OwnershipMode, Path, Unavailable};
use crate::ast::{CallArg, Expr, Span, Spanned};
use crate::ast_walk::any_expr_in_expr;
use incan_semantics_core::CanonicalSymbolId;

impl Builder<'_> {
    /// Bind a closure to a new local: read each captured Copy value, then borrow the one captured borrow for as long as
    /// the local is called.
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
        if any_expr_in_expr(&closure.node, |expression| matches!(expression, Expr::SelfExpr)) {
            return Err(refuse("a closure reading the receiver"));
        }
        let captures = self
            .checker
            .closure_captures(closure.span)
            .ok_or_else(|| refuse("closure has no checked captures"))?
            .to_vec();
        let mut identities = Vec::new();
        let mut borrowed: Option<Path> = None;
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
            let path = match self.aliases.get(&identity) {
                // An annotated borrowed local is captured as a reborrow of its own loan.
                Some(path) if self.annotated_borrow(&identity) => path.clone(),
                Some(_) => return Err(refuse("a closure capturing a view")),
                None => Path {
                    place: self.root(&identity, closure.span)?,
                    through: None,
                },
            };
            if path.through.is_some() || self.borrowed_inputs.contains(&path.place.root) {
                if self.exclusive_path(&path) {
                    return Err(refuse("a closure capturing an exclusive borrow"));
                }
                // The native closure holds its captures by value, so a captured borrow is read by copy; a value
                // that is not Copy would need the closure to hold the reference itself.
                if !self.copy_value(&capture.ty) {
                    return Err(refuse("a closure capturing a borrowed value that is not Copy"));
                }
                if borrowed.replace(path).is_some() {
                    return Err(refuse("a closure capturing more than one borrow"));
                }
            } else if self.copy_value(&capture.ty) {
                let read = Event::Access {
                    place: path.place,
                    access: Access::Read,
                    through: None,
                };
                incoming = self.event(incoming, read, closure.span);
                reads.extend(incoming.iter().copied());
            } else {
                return Err(refuse("a closure capturing an owned value"));
            }
            identities.push(identity);
        }
        let held = match borrowed {
            Some(path) => {
                let loan = LoanId(self.graph.loans.len());
                self.graph.loans.push(Loan {
                    source: path.place.clone(),
                    kind: LoanKind::Shared,
                    writable: false,
                    parent: path.through,
                    span: closure.span,
                });
                incoming = self.event(incoming, Event::Borrow(loan), closure.span);
                if self.graph.local_loans.insert(local.clone(), loan).is_some() {
                    return Err(refuse("local declaration has more than one ownership destination"));
                }
                Some(Path {
                    place: path.place,
                    through: Some(loan),
                })
            }
            None => None,
        };
        let closure_record = ClosureCaptures {
            local: local.clone(),
            captures: identities,
            loan: held.as_ref().and_then(|path| path.through),
            reads,
        };
        self.graph.closures.insert(closure.span, closure_record);
        self.closures.insert(local, held);
        Ok(incoming)
    }

    /// The closure local a callee names, if it names one.
    pub(super) fn closure_callee(&self, callee: &Spanned<Expr>) -> Option<CanonicalSymbolId> {
        if !matches!(callee.node, Expr::Ident(_)) {
            return None;
        }
        let identity = self.checker.type_info.resolved_identity(callee.span)?;
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
        Ok(match self.closures.get(closure).cloned().flatten() {
            Some(path) => {
                let use_of_borrow = Event::Access {
                    place: path.place,
                    access: Access::Read,
                    through: path.through,
                };
                self.event(incoming, use_of_borrow, span)
            }
            None => incoming,
        })
    }

    /// Whether a local is declared `Borrowed[T]` or `BorrowedMut[T]`, rather than bound to a view a call returns.
    fn annotated_borrow(&self, identity: &CanonicalSymbolId) -> bool {
        self.checker
            .type_info
            .protocols
            .explicit_local_modes
            .get(identity)
            .is_some_and(|mode| matches!(mode.mode, OwnershipMode::Borrowed | OwnershipMode::BorrowedMut))
    }

    /// Whether a captured place is borrowed exclusively: through an exclusive loan, or as an exclusive input.
    fn exclusive_path(&self, path: &Path) -> bool {
        match path.through {
            Some(loan) => self
                .graph
                .loans
                .get(loan.0)
                .is_some_and(|loan| loan.kind == LoanKind::Exclusive),
            None => self
                .graph
                .permissions
                .get(&path.place.root)
                .is_some_and(|permission| permission.exclusive),
        }
    }
}
