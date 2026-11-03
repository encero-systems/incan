//! Native results that borrow their receiver. A native call whose selected signature borrows its receiver shared and
//! returns references with only the receiver's lifetime keeps a shared loan on the receiver's place after the call.
//! The new local or match arm binding that holds the result holds that loan: each read of it uses the loan, so the
//! place stays borrowed until the binding's last use. Moving the result out of its binding is refused.

use incan_semantics_core::CanonicalSymbolId;

use super::{Access, Builder, Event, Path, Root, Unavailable};
use crate::ast::{Expr, Span, Spanned};
use crate::symbols::ResolvedType;

impl Builder<'_> {
    /// Whether an expression is, through parentheses, a native call whose result borrows its receiver.
    pub(super) fn native_borrowing_call(&self, expression: &Spanned<Expr>) -> bool {
        let call = unparenthesized(expression);
        matches!(call.node, Expr::MethodCall(..)) && self.checker.type_info.native_borrowed_result(call.span)
    }

    /// Evaluate a native call whose result borrows its receiver, then borrow the receiver's place for the result, under
    /// the receiver's own parent loan.
    pub(super) fn evaluate_native_borrowing_call(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        let span = unparenthesized(expression).span;
        // A Copy referent is read by copy through the binding; a binding of any other referent needs its own reference.
        let referent = match self.checker.type_info.expr_type(span) {
            Some(ResolvedType::Generic(_, arguments)) => arguments.first().cloned(),
            other => other.cloned(),
        };
        if !referent.is_some_and(|referent| self.copy_value(&referent)) {
            return Err(Unavailable {
                span,
                reason: "a borrowed native result of a value that is not Copy needs a reference-holding binding".into(),
            });
        }
        self.holding_native_result = true;
        let evaluated = self.expression(expression, incoming, false);
        self.holding_native_result = false;
        let incoming = evaluated?;
        let receiver = self
            .graph
            .invocations
            .get(&span)
            .and_then(|invocation| invocation.receiver)
            .and_then(|loan| self.graph.loans.get(loan.0))
            .cloned()
            .ok_or(Unavailable {
                span,
                reason: "borrowed native result has no checked receiver loan".into(),
            })?;
        if !matches!(receiver.source.root, Root::Binding(_)) {
            return Err(Unavailable {
                span,
                reason: "a borrowed native result of a temporary cannot outlive its call".into(),
            });
        }
        let path = Path {
            place: receiver.source,
            through: receiver.parent,
        };
        let loan = self.call_loan(path.clone(), false, false, span);
        let incoming = self.event(incoming, Event::Borrow(loan), span);
        Ok((
            incoming,
            Path {
                place: path.place,
                through: Some(loan),
            },
        ))
    }

    /// Bind a new local or arm binding holding a borrowed native result: a fresh root that also holds the result's
    /// loan.
    pub(super) fn bind_dependent(
        &mut self,
        identity: CanonicalSymbolId,
        span: Span,
        path: Path,
    ) -> Result<(), Unavailable> {
        self.root(&identity, span)?;
        self.dependents.insert(identity, path);
        Ok(())
    }

    /// The loan a binding holding a borrowed native result holds, when an expression names one.
    pub(super) fn dependent_loan(&self, expression: &Spanned<Expr>) -> Option<Path> {
        if !matches!(expression.node, Expr::Ident(_)) {
            return None;
        }
        let identity = self.checker.type_info.resolved_identity(expression.span)?;
        self.dependents.get(identity).cloned()
    }

    /// A read of a binding holding a borrowed native result uses the loan it holds.
    pub(super) fn dependent_use(&mut self, expression: &Spanned<Expr>, incoming: Vec<usize>) -> Vec<usize> {
        match self.dependent_loan(expression) {
            Some(path) => self.event(
                incoming,
                Event::Access {
                    place: path.place,
                    access: Access::Read,
                    through: path.through,
                },
                expression.span,
            ),
            None => incoming,
        }
    }
}

/// An expression without its redundant parentheses.
fn unparenthesized(expression: &Spanned<Expr>) -> &Spanned<Expr> {
    match &expression.node {
        Expr::Paren(inner) => unparenthesized(inner),
        _ => expression,
    }
}
