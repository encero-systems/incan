//! Authenticate retained operation completion and operand dependencies before liveness consumes their edges.

use std::collections::BTreeSet;

use incan_semantics_core::HirSourceSpan;

use super::{Access, Event, Graph, InvalidGraph, LoanId, Place};

/// One borrowed operand retains its exact evaluated storage until the enclosing operation consumes it.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct OperandDependency {
    /// Full expansion-bearing source anchor of this operand, including explicit parentheses.
    pub operand: HirSourceSpan,
    /// Canonical owner and stored projection used by the operation.
    pub source: Place,
    /// Existing alias or operation-local read loan that protects this source.
    pub loan: LoanId,
    /// Exact final Read event; another occurrence at the same place is not a substitute.
    pub final_access: usize,
}

/// Completion follows operand evaluation; its contiguous read sequence keeps every dependency live through it.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct OperationInputs {
    /// Unique Nothing marker emitted after operand evaluation and before the retained reads.
    pub completion_node: usize,
    /// Dependencies in the operation's actual operand order, excluding independent Copy or fresh values.
    pub operands: Vec<OperandDependency>,
}

/// Check explicit relationships, never discover them by searching for another read at a similar source span.
pub(super) fn validate_operations(graph: &Graph) -> Result<(), InvalidGraph> {
    let mut claimed = BTreeSet::new();
    for (operation, inputs) in &graph.operations {
        let marker = graph
            .nodes
            .get(inputs.completion_node)
            .ok_or(InvalidGraph("operation completion is outside callable"))?;
        if inputs.operands.is_empty()
            || marker.span != *operation
            || !matches!(marker.event, Event::Nothing)
            || !claimed.insert(inputs.completion_node)
        {
            return Err(InvalidGraph("operation completion has no unique marker and operands"));
        }
        let mut previous = inputs.completion_node;
        let mut operands = BTreeSet::new();
        for dependency in &inputs.operands {
            let node = graph
                .nodes
                .get(dependency.final_access)
                .ok_or(InvalidGraph("operation access is outside callable"))?;
            if !operands.insert(dependency.operand)
                || !claimed.insert(dependency.final_access)
                || dependency.final_access != previous + 1
                || graph.nodes[previous].successors.as_slice() != [dependency.final_access]
                || node.span != dependency.operand
                || !contains(operation, &dependency.operand)
                || !matches!(&node.event, Event::Access { place, access: Access::Read, through: Some(loan) }
                    if place == &dependency.source && *loan == dependency.loan)
                || graph
                    .loans
                    .get(dependency.loan.0)
                    .is_none_or(|loan| !loan.source.contains(&dependency.source))
            {
                return Err(InvalidGraph("operation operand differs from its retained consumption"));
            }
            previous = dependency.final_access;
        }
        // Operand evaluation may contain nested operations, but none may occur after this operation completes.
        for (index, node) in graph.nodes.iter().enumerate().skip(inputs.completion_node + 1) {
            if contains(operation, &node.span)
                && !inputs
                    .operands
                    .iter()
                    .any(|dependency| dependency.final_access == index)
            {
                return Err(InvalidGraph("operation completes before an evaluated operand"));
            }
        }
    }
    Ok(())
}

/// Compare the full expansion owner before using a range to check an already retained relationship.
fn contains(outer: &HirSourceSpan, inner: &HirSourceSpan) -> bool {
    outer.expansion == inner.expansion && outer.start <= inner.start && inner.end <= outer.end
}
