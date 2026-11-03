//! Access projection for builtin carriers: `Ok`, `Err` and `Some` construction and statement `match` arms on a fresh
//! carrier or on a local still holding a carried view. Other place scrutinees, guards and other patterns need their
//! own checked binding access.

use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_semantics_core::CanonicalSymbolId;

use super::{Builder, Event, Path, Place, Unavailable};
use crate::ast::{CallArg, Expr, Literal, MatchArm, MatchBody, Pattern, PatternArg, Span, Spanned};

/// A local still holding a carried call: the payload carrying its view, and how many loops enclose its binding.
#[derive(Debug, Clone, Copy)]
pub(super) struct BoundCarrier {
    pub(super) payload: usize,
    pub(super) loop_depth: usize,
}

/// The carrier bookkeeping at a branch point: the locals still holding a carried call, each with its alias, and the
/// locals whose view was already taken out. Each branch starts from it, and the paths that continue join after.
#[derive(Clone)]
pub(super) struct CarrierState {
    bound: BTreeMap<CanonicalSymbolId, (BoundCarrier, Path)>,
    taken: BTreeSet<CanonicalSymbolId>,
}

impl Builder<'_> {
    /// The carrier bookkeeping as it stands, for a branch point.
    pub(super) fn carrier_state(&self) -> CarrierState {
        CarrierState {
            bound: self
                .carriers
                .iter()
                .filter_map(|(identity, carrier)| {
                    Some((identity.clone(), (*carrier, self.aliases.get(identity)?.clone())))
                })
                .collect(),
            taken: self.taken_carriers.clone(),
        }
    }

    /// Start a branch from `state`: carriers bound in an earlier branch are dropped, and those it took out return.
    pub(super) fn restore_carrier_state(&mut self, state: &CarrierState) {
        let stale: Vec<CanonicalSymbolId> = self
            .carriers
            .keys()
            .chain(&self.taken_carriers)
            .filter(|identity| !state.bound.contains_key(*identity))
            .cloned()
            .collect();
        for identity in stale {
            self.aliases.remove(&identity);
        }
        self.carriers.clear();
        for (identity, (carrier, path)) in &state.bound {
            self.carriers.insert(identity.clone(), *carrier);
            self.aliases.insert(identity.clone(), path.clone());
        }
        self.taken_carriers = state.taken.clone();
    }

    /// Join the branches that continue: a local whose view any of them took out is taken after the join, and the
    /// others stay bound. With no continuing branch, the code after is unreachable and keeps the entry state.
    pub(super) fn join_carrier_states(&mut self, entry: &CarrierState, ends: &[CarrierState]) {
        if ends.is_empty() {
            self.restore_carrier_state(entry);
            return;
        }
        let taken: BTreeSet<CanonicalSymbolId> = ends.iter().flat_map(|state| state.taken.iter().cloned()).collect();
        let mut bound = BTreeMap::new();
        for state in ends {
            for (identity, carrier) in &state.bound {
                if !taken.contains(identity) {
                    bound.entry(identity.clone()).or_insert_with(|| carrier.clone());
                }
            }
        }
        self.restore_carrier_state(&CarrierState { bound, taken });
    }

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
    /// call's result loan covers the arms: the binding of the variant holding the view (`Ok` or `Some`, or `Err` for
    /// a view carried in the error) aliases the caller's place under it, while the other arms hold no view, so the
    /// loan ends there unless a later use keeps it.
    pub(super) fn match_statement(
        &mut self,
        scrutinee: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
        incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let (evaluated, view) = if let Some(relation) = self.call_relation(scrutinee)?
            && let Some(payload) = relation.carried
        {
            let (evaluated, path) = self.evaluate_view_call(scrutinee, relation, incoming)?;
            (evaluated, Some((path, payload)))
        } else if let Some((identity, payload)) = self.bound_carrier(scrutinee) {
            let (evaluated, path) = self.take_bound_view(scrutinee, identity, incoming)?;
            (evaluated, Some((path, payload)))
        } else if self.is_taken_carrier(scrutinee) {
            return Err(super::taken_carrier(scrutinee.span));
        } else if fresh_scrutinee(scrutinee) {
            (self.expression(scrutinee, incoming, false)?, None)
        } else {
            return Err(Unavailable {
                span: scrutinee.span,
                reason: "match on a stored place needs checked binding access".into(),
            });
        };
        let mut outgoing = Vec::new();
        // Each arm starts from the carriers as the scrutinee left them; the arms that continue join after the match.
        let entry = self.carrier_state();
        let mut ends = Vec::new();
        for arm in arms {
            self.restore_carrier_state(&entry);
            if let Some(guard) = &arm.node.guard {
                return Err(Unavailable {
                    span: guard.span,
                    reason: "match guard needs checked binding access".into(),
                });
            }
            let binding = view.as_ref().and_then(|(path, payload)| {
                super::super::builtins::carried_view_binding(&arm.node.pattern, *payload).map(|binding| (path, binding))
            });
            match binding {
                Some((path, (name, span))) => self.view_arm_binding(name, span, path)?,
                None => self.arm_binding(&arm.node.pattern)?,
            }
            let continuing = match &arm.node.body {
                MatchBody::Expr(value) => self.expression(value, evaluated.clone(), false)?,
                MatchBody::Block(body) => self.block(body, evaluated.clone())?,
            };
            if !continuing.is_empty() {
                ends.push(self.carrier_state());
            }
            outgoing.extend(continuing);
        }
        self.join_carrier_states(&entry, &ends);
        Ok(outgoing)
    }

    /// A carried view's body returns the variant holding the view (`Ok` or `Some`, or `Err` for a view carried in the
    /// error) of its declared source place, a carried view of the same source from another call, or another variant
    /// or `None`, which carries no view. Any other value is a program error.
    pub(super) fn carried_return(
        &mut self,
        value: &Spanned<Expr>,
        carried: usize,
        exclusive: bool,
        source: &Place,
        incoming: Vec<usize>,
    ) -> Result<(), Unavailable> {
        match self.carrier_value(value) {
            Some((variant, arguments)) if super::super::builtins::variant_carries_view(variant, carried) => {
                let [CallArg::Positional(payload)] = arguments else {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "carrier construction needs one positional payload".into(),
                    });
                };
                return self.view_return(payload, exclusive, source, incoming);
            }
            Some(_) => {
                self.expression(value, incoming, true)?;
                return Ok(());
            }
            None => {}
        }
        if matches!(value.node, Expr::Literal(Literal::None)) {
            return Ok(());
        }
        if let Some(relation) = self
            .call_relation(value)?
            .filter(|relation| relation.carried == Some(carried))
        {
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
            if carried == 0 {
                "A returned carried view must return Ok of its declared source place, an error, or a carried view of \
                 the same source"
            } else {
                "A returned carried view must return Err of its declared source place, a success value, or a carried \
                 view of the same source"
            },
            value.span,
        ));
        self.expression(value, incoming, true)?;
        Ok(())
    }

    /// Take the view out of a local still holding a carried call, for a `match` or `?` on it. The local's read
    /// through its loan keeps the loan live until here, and taking the view consumes the local. The view reborrows
    /// that loan, keyed to the same call, so each loan keeps one local destination. Inside a loop the local's binding
    /// does not enclose, every iteration would take the view out of the same local, so that is refused.
    pub(super) fn take_bound_view(
        &mut self,
        carrier: &Spanned<Expr>,
        identity: CanonicalSymbolId,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        let bound = self.carriers.remove(&identity).ok_or(Unavailable {
            span: carrier.span,
            reason: "bound carried view has no carrier".into(),
        })?;
        if bound.loop_depth < self.loops.len() {
            return Err(Unavailable {
                span: carrier.span,
                reason: "a carried result bound outside this loop would be taken out on every iteration".into(),
            });
        }
        let path = self.aliases.get(&identity).cloned().ok_or(Unavailable {
            span: carrier.span,
            reason: "bound carried view has no source place".into(),
        })?;
        let carried = path
            .through
            .and_then(|loan| self.graph.loans.get(loan.0))
            .cloned()
            .ok_or(Unavailable {
                span: carrier.span,
                reason: "bound carried view has no result loan".into(),
            })?;
        let evaluated = self.expression(carrier, incoming, false)?;
        self.aliases.remove(&identity);
        self.taken_carriers.insert(identity);
        let exclusive = carried.kind == super::LoanKind::Exclusive;
        let view = self.call_loan(path.clone(), exclusive, carried.writable, carried.span);
        let evaluated = self.event(evaluated, Event::Borrow(view), carrier.span);
        Ok((
            evaluated,
            Path {
                place: path.place,
                through: Some(view),
            },
        ))
    }

    /// Whether an expression names, through parentheses, a local whose carried view was already taken out.
    fn is_taken_carrier(&self, expression: &Spanned<Expr>) -> bool {
        match &expression.node {
            Expr::Paren(inner) => self.is_taken_carrier(inner),
            Expr::Ident(_) => self
                .checker
                .type_info
                .resolved_identity(expression.span)
                .is_some_and(|identity| self.taken_carriers.contains(identity)),
            _ => false,
        }
    }

    /// The identity of a local still holding a carried view, through parentheses, with the payload carrying it.
    pub(super) fn bound_carrier(&self, expression: &Spanned<Expr>) -> Option<(CanonicalSymbolId, usize)> {
        match &expression.node {
            Expr::Paren(inner) => self.bound_carrier(inner),
            Expr::Ident(_) => {
                let identity = self.checker.type_info.resolved_identity(expression.span)?;
                let carrier = self.carriers.get(identity)?;
                Some((identity.clone(), carrier.payload))
            }
            _ => None,
        }
    }

    /// Bind the payload holding a carried view's call's view as a view local of the caller's place under the call's
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
