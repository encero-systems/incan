//! Authenticate producer-linked call intervals against canonical inputs and their source expressions.

use incan_semantics_core::{HirSourceSpan, IncanInputAccess, SemanticSourceTargetKind};

use super::{CheckedInputBody, TypeCheckInfo, flow};
use crate::ast::{CallArg, Expr, Spanned};
use crate::symbols::CallableParam;
use crate::typechecker::TypeChecker;

/// Check exact event links without searching for a similar event elsewhere in the body. Source coverage then
/// authenticates expression ownership and CFG execution; neither layer may replace the other.
pub(super) fn validate_invocation_events(
    facts: &TypeCheckInfo,
    graph: &flow::Graph,
    span: HirSourceSpan,
    invocation: &flow::Invocation,
    parameters: &[CallableParam],
    declaration_order: bool,
) -> Result<(), &'static str> {
    let mut end = None;
    let mut loans = Vec::new();
    match (invocation.receiver, &invocation.receiver_evaluation) {
        (Some(loan), Some(evaluation)) => {
            let source = graph.loans.get(loan.0).ok_or("receiver loan is absent")?;
            validate_evaluation(graph, source.span, evaluation, Some(&source.source), Some(loan), None)?;
            end = Some(evaluation.end_node);
            loans.push(loan);
        }
        (None, Some(evaluation)) if invocation.receiver_owned => {
            let receiver_span = graph
                .nodes
                .get(evaluation.start_node)
                .map(|node| node.span)
                .ok_or("owned receiver evaluation starts outside the body")?;
            let access = invocation
                .receiver_source
                .as_ref()
                .map(|_| {
                    let ty = facts
                        .expr_type(receiver_span)
                        .ok_or("owned receiver has no checked value type")?;
                    Ok::<_, &'static str>(if facts.checked_input_copy_value(&graph.owner, ty)? {
                        flow::Access::Read
                    } else {
                        flow::Access::Move
                    })
                })
                .transpose()?;
            validate_evaluation(
                graph,
                receiver_span,
                evaluation,
                invocation.receiver_source.as_ref(),
                None,
                access,
            )?;
            end = Some(evaluation.end_node);
        }
        (None, None) => {}
        _ => return Err("receiver evaluation and loan must be retained together"),
    }
    let mut previous_slot = None;
    for (position, argument) in invocation.arguments.iter().enumerate() {
        if !declaration_order {
            if position != argument.written_position {
                return Err("invocation arguments differ from their selected evaluation order");
            }
        } else if previous_slot.is_some_and(|slot| slot >= argument.slot) {
            return Err("invocation arguments differ from their selected evaluation order");
        }
        previous_slot = Some(argument.slot);
        let parameter = parameters.get(argument.slot).ok_or("input slot is absent")?;
        let expected = if argument.access == IncanInputAccess::Owned
            && !facts.checked_input_copy_value(&graph.owner, &parameter.ty)?
        {
            flow::Access::Move
        } else {
            flow::Access::Read
        };
        validate_evaluation(
            graph,
            argument.span,
            &argument.evaluation,
            argument.source.as_ref(),
            argument.loan,
            Some(expected),
        )?;
        if end.is_some_and(|previous| previous + 1 != argument.evaluation.start_node) {
            return Err("argument evaluation intervals are not consecutive");
        }
        end = Some(argument.evaluation.end_node);
        loans.extend(argument.loan);
        // Resolve the expression type as well as the selected parameter: a corrupt slot cannot change Copy
        // semantics even when the declaration's public access tag happens to agree.
        if argument.loan.is_none() && argument.source.is_some() {
            let ty = facts
                .expr_type(argument.span)
                .ok_or("stored argument has no checked value type")?;
            let source_copy = facts.checked_input_copy_value(&graph.owner, ty)?;
            if (expected == flow::Access::Move) != (argument.access == IncanInputAccess::Owned && !source_copy) {
                return Err("stored argument Copy semantics differ from its selected parameter");
            }
        }
    }
    marker(graph, invocation.completion_node, span)?;
    if end.is_some_and(|previous| previous + 1 != invocation.completion_node) {
        return Err("invocation completes before all input evaluations");
    }
    if loans.len() != invocation.final_accesses.len() {
        return Err("invocation final loan uses are incomplete");
    }
    for (offset, (id, index)) in loans.into_iter().zip(&invocation.final_accesses).enumerate() {
        if *index != invocation.completion_node + offset + 1 {
            return Err("invocation final loan uses do not follow completion");
        }
        let loan = graph.loans.get(id.0).ok_or("invocation loan is absent")?;
        let node = graph.nodes.get(*index).ok_or("invocation final use is outside body")?;
        if node.span != span
            || !matches!(&node.event, flow::Event::Access { place, access: flow::Access::Read, through: Some(actual) }
            if *actual == id && place == &loan.source)
        {
            return Err("invocation final use differs from its exact loan");
        }
    }
    Ok(())
}

/// Validate a complete bounded input interval, including literals that produce no value-access event.
fn validate_evaluation(
    graph: &flow::Graph,
    span: HirSourceSpan,
    evaluation: &flow::InputEvaluation,
    source: Option<&flow::Place>,
    loan: Option<flow::LoanId>,
    access: Option<flow::Access>,
) -> Result<(), &'static str> {
    marker(graph, evaluation.start_node, span)?;
    marker(graph, evaluation.end_node, span)?;
    if evaluation.start_node >= evaluation.end_node {
        return Err("input evaluation boundaries are reversed");
    }
    for node in &graph.nodes[evaluation.start_node + 1..evaluation.end_node] {
        if !contains(span, node.span) {
            return Err("input evaluation contains another expression's event");
        }
    }
    for (index, node) in graph.nodes.iter().enumerate() {
        if contains(span, node.span) && (index < evaluation.start_node || index > evaluation.end_node) {
            return Err("input evaluation omits events belonging to its source expression");
        }
    }
    match (source, evaluation.source_node) {
        (None, None) if loan.is_none() => {}
        (Some(source), Some(index)) if index == evaluation.end_node - 1 && index > evaluation.start_node => {
            let node = &graph.nodes[index];
            let valid = match (loan, &node.event) {
                (Some(expected), flow::Event::Borrow(actual)) => expected == *actual,
                (
                    None,
                    flow::Event::Access {
                        place, access: actual, ..
                    },
                ) => place == source && Some(*actual) == access,
                _ => false,
            };
            if !valid {
                return Err("input source event differs from its selected transfer or borrow");
            }
        }
        _ => return Err("input source and final evaluated event disagree"),
    }
    Ok(())
}

/// Marker links are explicit producer output, never recovered by scanning source ranges.
fn marker(graph: &flow::Graph, index: usize, span: HirSourceSpan) -> Result<(), &'static str> {
    let node = graph.nodes.get(index).ok_or("input marker is outside body")?;
    if node.span != span || !matches!(node.event, flow::Event::Nothing) {
        return Err("input marker differs from its source expression");
    }
    Ok(())
}

/// Expansion identity is part of containment; generated expressions cannot adopt another expansion's events.
fn contains(outer: HirSourceSpan, inner: HirSourceSpan) -> bool {
    outer.expansion == inner.expansion && outer.start <= inner.start && inner.end <= outer.end
}

impl CheckedInputBody<'_> {
    /// Reconstruct fixed argument association from the AST and selected canonical parameters. Persisted vector
    /// order and source spans alone cannot authorize another written argument or a different physical owner.
    pub(super) fn validate_call_expression(
        &self,
        span: HirSourceSpan,
        arguments: &[CallArg],
    ) -> Result<(), &'static str> {
        let call = self
            .facts
            .protocols
            .explicit_input_calls
            .get(&span)
            .ok_or("call has no selected input facts")?;
        let invocation = self
            .graph
            .invocations
            .get(&span)
            .ok_or("selected call has no retained evaluation")?;
        let mut paired = TypeChecker::arguments_with_parameter_slots(&call.parameters, arguments)
            .into_iter()
            .enumerate()
            .collect::<Vec<_>>();
        if call.kind != super::super::InputCallKind::Indirect && invocation.receiver_evaluation.is_none() {
            paired.sort_by_key(|(_, (_, slot))| *slot);
        }
        if paired.len() != invocation.arguments.len() {
            return Err("source call argument coverage is incomplete");
        }
        for ((written, (expression, slot)), actual) in paired.into_iter().zip(&invocation.arguments) {
            if actual.span != expression.span || Some(actual.slot) != slot || actual.written_position != written {
                return Err("retained input differs from its selected source argument");
            }
            let source = self.call_place(expression)?;
            match (source, actual.source.as_ref()) {
                (Some((source, through)), Some(actual_source)) if &source == actual_source => {
                    let index = actual
                        .evaluation
                        .source_node
                        .ok_or("stored input has no source event")?;
                    if let Some(loan) = actual.loan {
                        if self.graph.loans.get(loan.0).is_none_or(|loan| loan.parent != through) {
                            return Err("input borrow differs from its checked source loan");
                        }
                    } else if !matches!(&self.graph.nodes[index].event, flow::Event::Access { through: actual, .. } if *actual == through)
                    {
                        return Err("input transfer differs from its checked source loan");
                    }
                }
                (None, None) if actual.loan.is_none() => {}
                (
                    None,
                    Some(flow::Place {
                        root:
                            flow::Root::Temporary {
                                owner,
                                expression: actual_expression,
                                invocation: actual_invocation,
                            },
                        projection,
                    }),
                ) if actual.loan.is_some()
                    && owner == self.canonical_owner()
                    && *actual_expression == expression.span
                    && *actual_invocation == span
                    && projection.is_empty() => {}
                (None, Some(actual_source)) if actual.loan.is_some() => {
                    let loan = self
                        .graph
                        .loans
                        .get(actual.loan.ok_or("returned view input has no borrow")?.0)
                        .ok_or("returned view input borrow is absent")?;
                    let parent = loan
                        .parent
                        .and_then(|parent| self.graph.loans.get(parent.0))
                        .ok_or("returned view input has no result loan")?;
                    let inner = super::super::builtins::view_call(expression);
                    if &loan.source != actual_source
                        || parent.source != *actual_source
                        || parent.span != self.view_call_span(expression)
                        || !self.graph.invocations.contains_key(&inner.span)
                    {
                        return Err("returned view input differs from its checked result loan");
                    }
                }
                _ => return Err("input source differs from its canonical source expression"),
            }
        }
        Ok(())
    }

    /// Resolve ordinary owners and explicit aliases through existing checked binding and field identities.
    pub(super) fn call_place(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<(flow::Place, Option<flow::LoanId>)>, &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr => {
                let identity = self
                    .facts
                    .resolved_identity(expression.span)
                    .ok_or("input place has no canonical identity")?;
                // A closure local holds its closure value; its loan is the captured borrow, not an alias.
                if let Some(id) = self.graph.local_loans.get(identity)
                    && !self.graph.closures.values().any(|closure| &closure.local == identity)
                {
                    let loan = self.graph.loans.get(id.0).ok_or("input alias loan is absent")?;
                    return Ok(Some((loan.source.clone(), Some(*id))));
                }
                Ok(Some((
                    flow::Place {
                        root: flow::Root::Binding(identity.clone()),
                        projection: Vec::new(),
                    },
                    None,
                )))
            }
            Expr::Paren(inner) => self.call_place(inner),
            Expr::Field(base, _) => {
                let (mut place, through) = self.call_place(base)?.ok_or("input field has no stored owner")?;
                let field = self
                    .facts
                    .resolved_identity(expression.span)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
                    .ok_or("input field has no canonical declaration")?;
                if self.facts.computed_property_access(expression.span).is_some() {
                    return Err("input field is a computed property");
                }
                place.projection.push(flow::Projection::Field(field.clone()));
                Ok(Some((place, through)))
            }
            _ => Ok(None),
        }
    }
}
