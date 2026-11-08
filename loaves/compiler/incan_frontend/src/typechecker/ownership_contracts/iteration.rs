//! Projection of direct borrowed-list iteration conflicts from the canonical ownership graph.

use incan_lang::lang::types::collections::{self, CollectionTypeId};

use super::flow::{Access, Conflict, ConflictKind, Event, Graph, LoanId};
use super::report::display_place;
use crate::diagnostics::{CompileError, errors};
use crate::symbols::ResolvedType;
use crate::typechecker::TypeCheckInfo;

/// Project the historical T0121 refusal only when the conflicting graph loan is the poll loan of a direct list loop
/// and the operation replaces that list or one of its owning parents.
pub(super) fn conflict(graph: &Graph, facts: &TypeCheckInfo, conflict: &Conflict) -> Option<CompileError> {
    let ConflictKind::ActiveLoan(loan) = conflict.kind else {
        return None;
    };
    let Event::Access {
        place,
        access: Access::Replace,
        through: None,
    } = &graph.nodes.get(conflict.node)?.event
    else {
        return None;
    };
    let borrowed = graph.loans.get(loan.0)?;
    if !place.contains(&borrowed.source) || !is_direct_list_iteration_loan(graph, facts, loan) {
        return None;
    }
    Some(errors::iterated_place_replacement(
        &display_place(&borrowed.source),
        graph.nodes[conflict.node].span,
    ))
}

/// Authenticate a direct list iteration loan from checked type facts and the graph builder's loop-header fact.
/// Ordinary call borrows can share the same source and span shapes but are not recorded as iteration loans.
fn is_direct_list_iteration_loan(graph: &Graph, facts: &TypeCheckInfo, loan: LoanId) -> bool {
    let Some(borrowed) = graph.loans.get(loan.0) else {
        return false;
    };
    if !matches!(facts.expr_type(borrowed.span), Some(ResolvedType::Generic(name, _))
        if collections::from_str(name) == Some(CollectionTypeId::List))
    {
        return false;
    }
    graph.direct_iteration_loans.contains(&loan)
}
