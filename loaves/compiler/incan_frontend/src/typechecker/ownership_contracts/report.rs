//! Source diagnostic projection of validated ownership conflicts. Related locations follow graph reachability,
//! including reborrow ancestry, rather than selecting a later textual occurrence of the same name.

use std::collections::{BTreeSet, VecDeque};

use super::flow::{Access, Analysis, ConflictKind, Event, Graph, LoanId, Root};
use crate::ast::Span;
use crate::diagnostics::CompileError;
use crate::typechecker::TypeCheckInfo;

/// Render conflicts only from a successfully validated graph. The result retains structured locations for CLI,
/// editor and serialized diagnostic projections; native storage selection is not involved.
pub(super) fn conflicts(graph: &Graph, analysis: &Analysis, facts: &TypeCheckInfo) -> Vec<CompileError> {
    analysis
        .conflicts
        .iter()
        .map(|conflict| {
            let operation = &graph.nodes[conflict.node];
            if let Some(loan) = builtin_conflict_loan(graph, analysis, facts, conflict) {
                return builtin_result_conflict(graph, analysis, conflict.node, loan);
            }
            let message = match conflict.kind {
                ConflictKind::Permission => "This operation requires access not granted by its ownership contract",
                ConflictKind::ActiveLoan(_) => "This operation conflicts with a live borrow of the same storage",
                ConflictKind::Moved(_) => "This storage may already have been moved on a path reaching this operation",
                ConflictKind::UnavailableLoan(_) => {
                    "This borrow has not been established on every path reaching its use"
                }
            };
            let error = CompileError::type_error(message.into(), operation.span).with_stable_code("INCAN-T0126");
            match conflict.kind {
                ConflictKind::Permission => permission_origin(error, graph, &operation.event),
                ConflictKind::Moved(origin) => error
                    .with_related_span(graph.nodes[origin].span, "ownership transferred here")
                    .with_hint("Use the value before transferring it, or reinitialize the moved place before using it"),
                ConflictKind::UnavailableLoan(loan) => error
                    .with_related_span(graph.loans[loan.0].span, "this borrow is established here")
                    .with_hint("Establish the borrow on every path that reaches its use"),
                ConflictKind::ActiveLoan(loan) => {
                    let mut error = error.with_related_span(graph.loans[loan.0].span, "conflicting borrow starts here");
                    if let Some(span) = dependent_use(graph, analysis, conflict.node, loan) {
                        error = error.with_related_span(span, "this reachable use keeps the borrow live");
                    }
                    error.with_hint("Finish the dependent use before accessing the overlapping place")
                }
            }
        })
        .collect()
}

/// Select the retained builtin loan responsible for this conflict, including a permission or moved conflict emitted
/// at the same synthetic escape boundary.
fn builtin_conflict_loan(
    graph: &Graph,
    analysis: &Analysis,
    facts: &TypeCheckInfo,
    conflict: &super::flow::Conflict,
) -> Option<LoanId> {
    let direct = match conflict.kind {
        ConflictKind::ActiveLoan(loan) | ConflictKind::UnavailableLoan(loan) => Some(loan),
        ConflictKind::Permission | ConflictKind::Moved(_) => match graph.nodes[conflict.node].event {
            Event::Access {
                through: Some(loan), ..
            } => Some(loan),
            _ => analysis.conflicts.iter().find_map(|candidate| {
                (candidate.node == conflict.node)
                    .then_some(candidate)
                    .and_then(|candidate| match candidate.kind {
                        ConflictKind::ActiveLoan(loan) => Some(loan),
                        _ => None,
                    })
            }),
        },
    }?;
    let mut loan = Some(direct);
    while let Some(id) = loan {
        if super::builtins::lazy_result_loan(graph, facts, id) {
            return Some(id);
        }
        loan = graph.loans.get(id.0)?.parent;
    }
    None
}

/// Explain a lazy builtin conflict using the source place, the conflicting action and the dependent later use.
fn builtin_result_conflict(graph: &Graph, analysis: &Analysis, node: usize, loan: LoanId) -> CompileError {
    let operation = &graph.nodes[node];
    let retained = &graph.loans[loan.0];
    let source = display_place(&retained.source);
    let action = match operation.event {
        Event::Borrow(_) => "borrow",
        Event::Access {
            access: Access::Read, ..
        } => "read",
        Event::Access {
            access: Access::Write, ..
        } => "mutate",
        Event::Access {
            access: Access::Replace,
            ..
        } => "replace",
        Event::Access {
            access: Access::Move, ..
        } => "transfer",
        Event::Nothing => "access",
    };
    let mut error = CompileError::type_error(
        format!("Cannot {action} `{source}` while a lazy result still reads it"),
        operation.span,
    )
    .with_stable_code("INCAN-T0001")
    .with_related_span(retained.span, format!("the lazy result starts reading `{source}` here"));
    if let Some(span) = dependent_use(graph, analysis, node, loan) {
        error = error.with_related_span(span, "this later use keeps the lazy result live");
    }
    error.with_hint(format!(
        "Finish the iteration before changing `{source}`, or write `{source}.clone()` to make an independent copy visible"
    ))
}

/// Render a canonical source place without reconstructing it from source spelling.
fn display_place(place: &super::flow::Place) -> String {
    let mut value = match &place.root {
        Root::Binding(identity) => identity.declaration_name.clone(),
        Root::Temporary { .. } => "temporary".into(),
    };
    for projection in &place.projection {
        match projection {
            super::flow::Projection::Field(identity) => {
                value.push('.');
                value.push_str(&identity.declaration_name);
            }
            super::flow::Projection::Element(_) => value.push_str("[item]"),
        }
    }
    value
}

/// Explain the exact source grant used for permission checking. A reborrow derives access from its parent rather
/// than upgrading to its original owner's permissions; local mutability cannot grant shared storage writes.
fn permission_origin(error: CompileError, graph: &Graph, event: &Event) -> CompileError {
    let (place, through, request) = match event {
        Event::Borrow(id) => {
            let loan = &graph.loans[id.0];
            (
                &loan.source,
                loan.parent,
                "Borrowing does not upgrade the source's access permission",
            )
        }
        Event::Access { place, access, through } => (
            place,
            *through,
            match access {
                Access::Read => "Preserve the source's declared access permission",
                Access::Write | Access::Replace => "Writing requires a writable owner or a writable exclusive borrow",
                Access::Move => "A borrowed place does not grant ownership transfer",
            },
        ),
        Event::Nothing => return error,
    };
    let error = match through {
        Some(loan) => error.with_related_span(graph.loans[loan.0].span, "access is granted by this borrow"),
        None => match &place.root {
            Root::Binding(identity) => {
                error.with_related_declaration(identity.clone(), "access is granted by this declaration")
            }
            Root::Temporary { expression, .. } => {
                error.with_related_span(*expression, "temporary storage is created here")
            }
        },
    };
    error.with_hint(request)
}

/// Find one reachable dependent access that witnesses a conflicting loan's liveness. Traversal is restricted to
/// live paths and terminates across loop back-edges; reborrow accesses also keep their ancestors live.
fn dependent_use(graph: &Graph, analysis: &Analysis, conflict: usize, loan: LoanId) -> Option<Span> {
    let mut pending: VecDeque<_> = graph.nodes[conflict].successors.iter().copied().collect();
    let mut visited = BTreeSet::new();
    while let Some(index) = pending.pop_front() {
        if !visited.insert(index) || !analysis.live_before[index].contains(&loan) {
            continue;
        }
        let node = &graph.nodes[index];
        if let Event::Access {
            through: Some(used), ..
        } = node.event
        {
            let mut ancestor = Some(used);
            while let Some(id) = ancestor {
                if id == loan {
                    return Some(node.span);
                }
                ancestor = graph.loans[id.0].parent;
            }
        }
        pending.extend(node.successors.iter().copied());
    }
    None
}
