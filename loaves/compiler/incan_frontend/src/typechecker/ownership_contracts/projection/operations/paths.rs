//! Authenticate execution edges from the admitted source statements, including lazy assertion failure messages.

use incan_semantics_core::HirSourceSpan;

use super::super::super::flow;
use crate::ast::{AssertKind, FunctionDecl, Statement};

/// Source event regions retain their producer order, but only these reconstructed edges establish execution.
/// This validator does not infer ownership from spans; canonical access and loan validation owns that contract.
pub(super) fn execution_order(graph: &flow::Graph, function: &FunctionDecl) -> Result<Vec<usize>, &'static str> {
    let entry = graph.nodes.first().ok_or("operation execution has no entry")?;
    if graph.entry != 0 || entry.span != graph.owner.declaration_span || !matches!(entry.event, flow::Event::Nothing) {
        return Err("operation execution has no canonical source entry");
    }
    let mut paths = SourcePaths {
        graph,
        expected: vec![Vec::new(); graph.nodes.len()],
        next: 1,
    };
    let mut incoming = vec![0];
    for statement in &function.body {
        if incoming.is_empty() {
            break;
        }
        incoming = match &statement.node {
            Statement::Pass => incoming,
            Statement::Assignment(_)
            | Statement::CompoundAssignment(_)
            | Statement::FieldAssignment(_)
            | Statement::Expr(_) => paths.region(statement.span, incoming),
            Statement::Return(value) => {
                if let Some(value) = value {
                    let _ = paths.region(value.span, incoming);
                }
                Vec::new()
            }
            Statement::Assert(assertion) => {
                let AssertKind::Condition(condition) = &assertion.kind else {
                    return Err("operation execution requires an ordinary assertion");
                };
                let success = paths.region(condition.span, incoming);
                if let Some(message) = &assertion.message {
                    // Failure terminates after its message. Even a zero-event message must not replace the
                    // independent success continuation with a fabricated branch node.
                    let _ = paths.region(message.span, success.clone());
                }
                success
            }
            _ => return Err("operation execution requires the admitted statement profile"),
        };
    }
    if paths.next != graph.nodes.len() {
        return Err("operation execution contains events outside reachable source regions");
    }
    for (node, expected) in graph.nodes.iter().zip(paths.expected) {
        if node.successors != expected {
            return Err("operation execution edges differ from the checked source paths");
        }
    }
    // The builder appends events in source-region order. Exact edge validation above authenticates that order;
    // nodes on a failure-only assertion path are not successors of the independent successful continuation.
    Ok((0..graph.nodes.len()).collect())
}

/// Consume complete event regions in AST order and reconstruct their outgoing edges without trusting stored edges.
struct SourcePaths<'a> {
    graph: &'a flow::Graph,
    expected: Vec<Vec<usize>>,
    next: usize,
}

impl SourcePaths<'_> {
    /// Ordinary expressions execute their retained events sequentially. Empty literal regions preserve incoming
    /// predecessors, exactly as the source graph producer does; no synthetic evaluation node is invented.
    fn region(&mut self, span: HirSourceSpan, mut incoming: Vec<usize>) -> Vec<usize> {
        while self.graph.nodes.get(self.next).is_some_and(|node| {
            node.span.expansion == span.expansion && span.start <= node.span.start && node.span.end <= span.end
        }) {
            for predecessor in incoming {
                if !self.expected[predecessor].contains(&self.next) {
                    self.expected[predecessor].push(self.next);
                }
            }
            incoming = vec![self.next];
            self.next += 1;
        }
        incoming
    }
}
