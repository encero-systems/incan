//! Access projection for builtin carriers: `Ok`, `Err` and `Some` construction and statement `match` arms on a fresh
//! carrier. Place scrutinees, guards and other patterns need their own checked binding access.

use incan_lang::lang::surface::constructors::{self, ConstructorId};

use super::{Builder, Event, Path, Place, Unavailable};
use crate::ast::{CallArg, Expr, Literal, MatchArm, MatchBody, Pattern, PatternArg, Span, Spanned};

impl Builder<'_> {
    /// A bare `Ok`, `Err` or `Some` that resolves to the builtin constructor, through the shared admission definition.
    pub(super) fn carrier_constructor(&self, callee: &Spanned<Expr>) -> bool {
        super::super::builtins::carrier_constructor(&self.checker.type_info, callee).is_some()
    }

    /// The builtin carrier construction a value applies, through the shared admission definition.
    fn carrier_value<'a>(&self, value: &'a Spanned<Expr>) -> Option<(ConstructorId, &'a [CallArg])> {
        super::super::builtins::carrier_construction(&self.checker.type_info, value)
    }

    /// A builtin carrier wraps one payload in fresh storage. Like a model field, the payload is fresh or Copy until
    /// constructed values gain checked transfer materialization; a call's result is fresh storage as well.
    pub(super) fn carrier_construction(
        &mut self,
        arguments: &[CallArg],
        span: Span,
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let [CallArg::Positional(payload)] = arguments else {
            return Err(Unavailable {
                span,
                reason: "carrier construction needs one positional payload".into(),
            });
        };
        if !self.fresh_or_copy_field(payload) && !matches!(payload.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return Err(Unavailable {
                span: payload.span,
                reason: "non-Copy carrier payload needs checked ownership transfer materialization".into(),
            });
        }
        self.expression(payload, incoming, false)
    }

    /// A statement `match` evaluates its fresh scrutinee once. Every arm starts from that evaluation and binds its
    /// payload as a fresh owner, and the arms that continue join after the match. On a call carrying a view, the
    /// call's result loan covers the arms: an `Ok` or `Some` binding aliases the caller's place under it, while an
    /// error or `None` arm holds no view, so the loan ends there unless a later use keeps it.
    pub(super) fn match_statement(
        &mut self,
        scrutinee: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let (evaluated, view) = match self.call_relation(scrutinee)?.filter(|relation| relation.carried) {
            Some(relation) => {
                let (evaluated, path) = self.evaluate_view_call(scrutinee, relation, incoming)?;
                (evaluated, Some(path))
            }
            None if fresh_scrutinee(scrutinee) => (self.expression(scrutinee, incoming, false)?, None),
            None => {
                return Err(Unavailable {
                    span: scrutinee.span,
                    reason: "match on a stored place needs checked binding access".into(),
                });
            }
        };
        let mut outgoing = Vec::new();
        for arm in arms {
            if let Some(guard) = &arm.node.guard {
                return Err(Unavailable {
                    span: guard.span,
                    reason: "match guard needs checked binding access".into(),
                });
            }
            match (&view, super::super::builtins::carried_view_binding(&arm.node.pattern)) {
                (Some(path), Some((name, span))) => self.view_arm_binding(name, span, path)?,
                _ => self.arm_binding(&arm.node.pattern)?,
            }
            outgoing.extend(match &arm.node.body {
                MatchBody::Expr(value) => self.expression(value, evaluated.clone(), false)?,
                MatchBody::Block(body) => self.block(body, evaluated.clone())?,
            });
        }
        Ok(outgoing)
    }

    /// A carried view's body returns `Ok` or `Some` of its declared source place, a carried view of the same source
    /// from another call, or an error or `None`, which carries no view. Any other value is a program error.
    pub(super) fn carried_return(
        &mut self,
        value: &Spanned<Expr>,
        exclusive: bool,
        source: &Place,
        incoming: Vec<usize>,
    ) -> Result<(), Unavailable> {
        match self.carrier_value(value) {
            Some((ConstructorId::Ok | ConstructorId::Some, arguments)) => {
                let [CallArg::Positional(payload)] = arguments else {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "carrier construction needs one positional payload".into(),
                    });
                };
                return self.view_return(payload, exclusive, source, incoming);
            }
            Some((ConstructorId::Err, _)) => {
                self.expression(value, incoming, true)?;
                return Ok(());
            }
            _ => {}
        }
        if matches!(value.node, Expr::Literal(Literal::None)) {
            return Ok(());
        }
        if let Some(relation) = self.call_relation(value)?.filter(|relation| relation.carried) {
            let (incoming, path) = self.evaluate_view_call(value, relation, incoming)?;
            if path.place == *source {
                let loan = self.call_loan(path, exclusive, exclusive, value.span);
                self.event(incoming, Event::Borrow(loan), value.span);
            } else {
                self.view_errors.push(super::view_error(
                    "A returned carried view must carry its declared source place; this call carries a view of a \
                     different place",
                    value.span,
                ));
            }
            return Ok(());
        }
        self.view_errors.push(super::view_error(
            "A returned carried view must return Ok of its declared source place, an error, or a carried view of the \
             same source",
            value.span,
        ));
        self.expression(value, incoming, true)?;
        Ok(())
    }

    /// Bind an `Ok` or `Some` payload of a carried view's call as a view local of the caller's place under the call's
    /// result loan, as `view = call()?` binds it.
    fn view_arm_binding(&mut self, name: &str, span: Span, path: &Path) -> Result<(), Unavailable> {
        let identity = self
            .checker
            .type_info
            .resolved_write_identity(span, name)
            .cloned()
            .ok_or(Unavailable {
                span,
                reason: "missing checked match binding".into(),
            })?;
        let loan = path.through.ok_or(Unavailable {
            span,
            reason: "carried view has no result loan".into(),
        })?;
        if self.graph.local_loans.insert(identity.clone(), loan).is_some() {
            return Err(Unavailable {
                span,
                reason: "match binding has more than one ownership destination".into(),
            });
        }
        self.aliases.insert(identity, path.clone());
        Ok(())
    }

    /// Register the one fresh owner a carrier pattern binds, as a loop item is registered: a root, no write event.
    fn arm_binding(&mut self, pattern: &Spanned<Pattern>) -> Result<(), Unavailable> {
        let unsupported = || Unavailable {
            span: pattern.span,
            reason: "match pattern needs checked binding access".into(),
        };
        let arguments = match &pattern.node {
            Pattern::Wildcard | Pattern::Literal(Literal::None) => return Ok(()),
            Pattern::Constructor(name, arguments) => match constructors::from_str(&name.node) {
                Some(ConstructorId::None) if arguments.is_empty() => return Ok(()),
                Some(ConstructorId::Ok | ConstructorId::Err | ConstructorId::Some) => arguments,
                _ => return Err(unsupported()),
            },
            _ => return Err(unsupported()),
        };
        let [PatternArg::Positional(payload)] = arguments.as_slice() else {
            return Err(unsupported());
        };
        match &payload.node {
            Pattern::Wildcard => Ok(()),
            Pattern::Binding(name) => {
                let identity = self
                    .checker
                    .type_info
                    .resolved_write_identity(payload.span, name)
                    .cloned()
                    .ok_or(Unavailable {
                        span: payload.span,
                        reason: "missing checked match binding".into(),
                    })?;
                self.root(&identity, payload.span).map(|_| ())
            }
            _ => Err(unsupported()),
        }
    }
}

/// A call's or constructor's result is fresh storage the arms may own; a stored place would lend or give up its
/// payload, which needs checked binding access.
fn fresh_scrutinee(scrutinee: &Spanned<Expr>) -> bool {
    match &scrutinee.node {
        Expr::Call(..) | Expr::MethodCall(..) | Expr::Literal(_) => true,
        Expr::Paren(inner) => fresh_scrutinee(inner),
        _ => false,
    }
}
