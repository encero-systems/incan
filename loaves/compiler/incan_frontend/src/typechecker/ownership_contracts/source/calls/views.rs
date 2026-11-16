//! Returned-view and native-result call relations and their evaluated source places.

use super::super::{
    Builder, Event, LoanId, LoanKind, OwnershipMode, Path, Place, Projection, Root, SourceSlot, Unavailable,
    ViewRelation, ViewSource, error_view_try,
};
use crate::ast::{Expr, Receiver, Spanned};
use crate::typechecker::ownership_contracts::{InputCallKind, builtins, native};
use incan_semantics_core::IncanInputAccess;

impl Builder<'_> {
    /// The place view an expression produces: a selected call returning a direct view, or `?` taking a carried view
    /// out of its `Result` or `Option` success payload. A call returning a carried view is itself a carrier value, not
    /// a place. `?` on a call whose error carries the view would pass that view on out of this body, and `?` in a body
    /// whose own error carries its view would return another error; neither executes yet.
    pub(in crate::typechecker::ownership_contracts::source) fn view_relation(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<ViewRelation>, Unavailable> {
        match &expression.node {
            Expr::Paren(inner) => self.view_relation(inner),
            Expr::Try(inner) => match self.call_relation(inner)? {
                Some(relation) if relation.carried == Some(1) => Err(Unavailable {
                    span: expression.span,
                    reason: "`?` passing on an error that carries a view".into(),
                }),
                _ if self.view_carried == Some(1) => Err(error_view_try(expression.span)),
                Some(relation) if relation.carried == Some(0) => Ok(Some(ViewRelation {
                    carried: None,
                    ..relation
                })),
                _ => Ok(None),
            },
            _ => Ok(self
                .call_relation(expression)?
                .filter(|relation| relation.component.is_empty())),
        }
    }

    /// The place view an expression produces, from a call as `view_relation` selects it or from `?` on a local still
    /// holding a carried call. `?` on such a local takes the view out as `?` on the call does.
    pub(in crate::typechecker::ownership_contracts::source) fn view_source(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<ViewSource>, Unavailable> {
        if let Some(relation) = self.view_relation(expression)? {
            return Ok(Some(ViewSource::Call(relation)));
        }
        match &expression.node {
            Expr::Paren(inner) => self.view_source(inner),
            Expr::Try(inner) => {
                let Some((identity, payload)) = self.bound_carrier(inner) else {
                    return Ok(None);
                };
                if payload != 0 {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "`?` passing on an error that carries a view".into(),
                    });
                }
                if self.view_carried == Some(1) {
                    return Err(error_view_try(expression.span));
                }
                Ok(Some(ViewSource::Carrier(identity)))
            }
            _ => Ok(None),
        }
    }

    /// Evaluate the place view `view_source` selected, then borrow the caller's place under its result loan.
    pub(in crate::typechecker::ownership_contracts::source) fn evaluate_view_source(
        &mut self,
        expression: &Spanned<Expr>,
        source: ViewSource,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        match (source, &expression.node) {
            (ViewSource::Call(relation), _) => self.evaluate_view_call(expression, relation, incoming),
            (source @ ViewSource::Carrier(_), Expr::Paren(inner)) => self.evaluate_view_source(inner, source, incoming),
            (ViewSource::Carrier(identity), Expr::Try(inner)) => self.take_bound_view(inner, identity, incoming),
            (ViewSource::Carrier(_), _) => Err(Unavailable {
                span: expression.span,
                reason: "bound carried view needs `?` or a match to take the view out".into(),
            }),
        }
    }

    /// The returned view relation of a selected call itself, direct or carried, read from its callee's contract
    /// without its body. Await and grouping preserve the selected call's relationship.
    pub(in crate::typechecker::ownership_contracts::source) fn call_relation(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<ViewRelation>, Unavailable> {
        let expression = builtins::view_call(expression);
        match &expression.node {
            Expr::Call(..) | Expr::MethodCall(..) => {}
            _ => return Ok(None),
        }
        let span = expression.span;
        let Some(call) = self.checker.type_info.protocols.explicit_input_calls.get(&span) else {
            return Ok(None);
        };
        let refusal = |reason: &'static str| Unavailable {
            span,
            reason: reason.into(),
        };
        let contract = self.checker.type_info.selected_input_contract(call).map_err(refusal)?;
        Ok(crate::ownership_metadata::returned_view(contract)
            .map_err(refusal)?
            .map(|view| ViewRelation {
                exclusive: view.access == IncanInputAccess::Exclusive,
                slot: view.source.slot.clone(),
                fields: view.source.fields.clone(),
                component: view.component.to_vec(),
                carried: view.carried,
            }))
    }

    /// The retained dependency declared by a selected top-level `Owned[T] from source` result. Its source input
    /// determines whether the result keeps shared or exclusive access; the owned result itself is ordinary value
    /// storage, not an alias of that source.
    pub(in crate::typechecker::ownership_contracts::source) fn owned_relation(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<ViewRelation>, Unavailable> {
        let expression = builtins::view_call(expression);
        if !matches!(expression.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return Ok(None);
        }
        let span = expression.span;
        let Some(call) = self.checker.type_info.protocols.explicit_input_calls.get(&span) else {
            return Ok(None);
        };
        let refusal = |reason: &'static str| Unavailable {
            span,
            reason: reason.into(),
        };
        let contract = self.checker.type_info.selected_input_contract(call).map_err(refusal)?;
        let Some((_, source)) = crate::ownership_metadata::owned_result_source(contract).map_err(refusal)? else {
            return Ok(None);
        };
        let exclusive = match source.slot {
            SourceSlot::Receiver => call.receiver == Some(Receiver::Mutable),
            SourceSlot::Parameter(slot) => contract
                .and_then(|contract| {
                    contract
                        .modes
                        .iter()
                        .find(|mode| mode.component.parameter == Some(slot) && mode.component.children.is_empty())
                })
                .is_some_and(|mode| mode.mode == OwnershipMode::BorrowedMut),
        };
        Ok(Some(ViewRelation {
            exclusive,
            slot: source.slot.clone(),
            fields: source.fields.clone(),
            component: Vec::new(),
            carried: None,
        }))
    }

    /// The exact retained owners of a native call result, including a builtin carrier payload when one holds the
    /// borrowed value. Static results return an empty source set and need no caller-owned loan.
    pub(in crate::typechecker::ownership_contracts::source) fn native_result_relation(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<native::NativeResultDependencies>, Unavailable> {
        let expression = builtins::view_call(expression);
        if !matches!(expression.node, Expr::Call(..) | Expr::MethodCall(..)) {
            return Ok(None);
        }
        let is_native = self
            .checker
            .type_info
            .protocols
            .explicit_input_calls
            .get(&expression.span)
            .is_some_and(|call| call.kind == InputCallKind::Native);
        if !is_native {
            return Ok(None);
        }
        self.checker
            .native_result_dependencies(expression.span)
            .map_err(|reason| Unavailable {
                span: expression.span,
                reason,
            })
    }

    /// Evaluate a native call once, then retain one result loan for every rustc-proven caller-owned input. The
    /// invocation's temporary loans end with the call; each result loan starts from the same admitted source place.
    pub(in crate::typechecker::ownership_contracts::source) fn evaluate_native_result(
        &mut self,
        expression: &Spanned<Expr>,
        relation: &native::NativeResultDependencies,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Vec<LoanId>), Unavailable> {
        let expression = builtins::view_call(expression);
        let span = expression.span;
        let incoming = match &expression.node {
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.apply_call(Some(receiver), arguments, span, incoming)?
            }
            Expr::Call(callee, _, arguments) if matches!(callee.node, Expr::Ident(_)) => {
                self.apply_call(None, arguments, span, incoming)?
            }
            _ => {
                return Err(Unavailable {
                    span,
                    reason: "native borrowed result needs a checked call".into(),
                });
            }
        };
        let invocation = self.graph.invocations.get(&span).cloned().ok_or(Unavailable {
            span,
            reason: "native borrowed result has no checked invocation".into(),
        })?;
        let mut incoming = incoming;
        let mut retained = Vec::with_capacity(relation.sources.len());
        for source in &relation.sources {
            let invocation_loans = match source {
                SourceSlot::Receiver => invocation
                    .receiver
                    .into_iter()
                    .chain(invocation.receiver_dependencies.iter().copied())
                    .collect::<Vec<_>>(),
                SourceSlot::Parameter(slot) => invocation
                    .arguments
                    .iter()
                    .find(|argument| argument.slot == *slot)
                    .and_then(|argument| argument.loan)
                    .into_iter()
                    .collect(),
            };
            if invocation_loans.is_empty() {
                return Err(Unavailable {
                    span,
                    reason: "native borrowed result source is not an admitted borrowed input".into(),
                });
            }
            for invocation_loan in invocation_loans {
                let base = self.graph.loans.get(invocation_loan.0).cloned().ok_or(Unavailable {
                    span,
                    reason: "native borrowed result source is not an admitted borrowed input".into(),
                })?;
                if !matches!(base.source.root, Root::Binding(_)) {
                    return Err(Unavailable {
                        span,
                        reason: format!("native call `{}` returns a borrow of temporary storage", relation.call),
                    });
                }
                let exclusive = base.kind == LoanKind::Exclusive;
                let loan = self.call_loan(
                    Path {
                        place: base.source,
                        through: base.parent,
                    },
                    exclusive,
                    base.writable,
                    span,
                );
                incoming = self.event(incoming, Event::Borrow(loan), span);
                retained.push(loan);
            }
        }
        Ok((incoming, retained))
    }

    /// Evaluate a call that returns a view, then borrow the substituted place: the invocation's receiver or
    /// argument source with the declared fields. The invocation's own loans cover the whole input during the call
    /// and end with it; afterwards only the narrower result loan stays live, under the source's parent loan.
    pub(in crate::typechecker::ownership_contracts::source) fn evaluate_view_call(
        &mut self,
        expression: &Spanned<Expr>,
        relation: ViewRelation,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Path), Unavailable> {
        let span = expression.span;
        let aggregate = !relation.component.is_empty();
        let incoming = match &expression.node {
            // `?` continues on the carried view's place; its error path leaves the body carrying no loan.
            Expr::Paren(inner) | Expr::Try(inner) => return self.evaluate_view_call(inner, relation, incoming),
            Expr::Surface(surface) if builtins::awaited(surface).is_some() => {
                let awaited = builtins::awaited(surface).ok_or(Unavailable {
                    span,
                    reason: "await has no returned-view expression".into(),
                })?;
                let call = builtins::view_call(awaited);
                self.awaited.insert(call.span);
                return self.evaluate_view_call(awaited, relation, incoming);
            }
            Expr::MethodCall(receiver, _, _, arguments) => {
                self.apply_call(Some(receiver), arguments, span, incoming)?
            }
            Expr::Call(callee, _, arguments) if matches!(callee.node, Expr::Ident(_)) => {
                self.apply_call(None, arguments, span, incoming)?
            }
            _ => {
                return Err(Unavailable {
                    span,
                    reason: "returned view call needs a checked callee".into(),
                });
            }
        };
        let invocation = self.graph.invocations.get(&span).ok_or(Unavailable {
            span,
            reason: "returned view call has no checked invocation".into(),
        })?;
        let base = match relation.slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .cloned()
        .ok_or(Unavailable {
            span,
            reason: "returned view source was not borrowed by its invocation".into(),
        })?;
        if matches!(base.source.root, Root::Temporary { .. }) && !aggregate {
            return Err(Unavailable {
                span,
                reason: "a returned view of a temporary cannot outlive its invocation".into(),
            });
        }
        let mut place = base.source;
        place
            .projection
            .extend(relation.fields.into_iter().map(Projection::Field));
        let loan = self.call_loan(
            Path {
                place: place.clone(),
                through: base.parent,
            },
            relation.exclusive,
            relation.exclusive,
            span,
        );
        let incoming = self.event(incoming, Event::Borrow(loan), span);
        Ok((
            incoming,
            Path {
                place,
                through: Some(loan),
            },
        ))
    }

    /// Evaluate an aggregate returned view and retain both its immediate source storage and every dependency stored
    /// by that source value. A cursor borrowing a selection therefore keeps the selection local alive and carries
    /// the selection's graph loan forward; neither dependency may be reconstructed from the nominal field shape.
    pub(in crate::typechecker::ownership_contracts::source) fn evaluate_aggregate_view_call(
        &mut self,
        expression: &Spanned<Expr>,
        relation: ViewRelation,
        incoming: Vec<usize>,
    ) -> Result<(Vec<usize>, Vec<LoanId>), Unavailable> {
        let source_slot = relation.slot.clone();
        let (mut incoming, path) = self.evaluate_view_call(expression, relation, incoming)?;
        let direct = path.through.ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view has no retained source loan".into(),
        })?;
        let call = builtins::view_call(expression);
        let invocation = self.graph.invocations.get(&call.span).ok_or(Unavailable {
            span: call.span,
            reason: "aggregate view call has no checked invocation".into(),
        })?;
        let source = match &source_slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == *slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .ok_or(Unavailable {
            span: call.span,
            reason: "aggregate view source was not borrowed by its invocation".into(),
        })?;
        let dependencies = match &source.source.root {
            Root::Binding(root) => self.graph.dependent_locals.get(root).cloned().unwrap_or_default(),
            Root::Temporary { .. } if source_slot == SourceSlot::Receiver => invocation.receiver_dependencies.clone(),
            Root::Temporary { .. } => Vec::new(),
        };
        let mut loans = vec![direct];
        for dependency in dependencies {
            let (next, _, loan) = self.carry_dependency(dependency, call.span, incoming)?;
            incoming = next;
            loans.push(loan);
        }
        Ok((incoming, loans))
    }

    /// Recover the substituted source place of an already evaluated view call without creating another result loan.
    /// Aggregate-return validation uses this after ordinary value evaluation so its graph matches the stored value's
    /// execution while still proving the declaration's exact `from` relationship.
    pub(in crate::typechecker::ownership_contracts::source) fn evaluated_view_call_place(
        &self,
        expression: &Spanned<Expr>,
        relation: &ViewRelation,
    ) -> Result<Place, Unavailable> {
        let expression = builtins::view_call(expression);
        let invocation = self.graph.invocations.get(&expression.span).ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view call has no checked invocation".into(),
        })?;
        let base = match relation.slot {
            SourceSlot::Receiver => invocation.receiver,
            SourceSlot::Parameter(slot) => invocation
                .arguments
                .iter()
                .find(|argument| argument.slot == slot)
                .and_then(|argument| argument.loan),
        }
        .and_then(|loan| self.graph.loans.get(loan.0))
        .ok_or(Unavailable {
            span: expression.span,
            reason: "aggregate view source was not borrowed by its invocation".into(),
        })?;
        let mut place = base.source.clone();
        place
            .projection
            .extend(relation.fields.iter().cloned().map(Projection::Field));
        Ok(place)
    }
}

