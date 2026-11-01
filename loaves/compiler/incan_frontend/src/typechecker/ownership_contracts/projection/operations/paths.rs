//! Authenticate execution edges from the admitted source statements, including lazy assertion failure messages.

use incan_semantics_core::HirSourceSpan;

use super::super::super::flow;
use crate::ast::{AssertKind, Condition, FunctionDecl, Spanned, Statement};

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
    let _ = paths.block(&function.body, vec![0])?;
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
    /// Follow source blocks and conditional joins independently of executable admission. A returning arm never
    /// supplies a join predecessor; empty arms retain their condition predecessors without synthetic nodes.
    fn block(
        &mut self,
        statements: &[Spanned<Statement>],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, &'static str> {
        for statement in statements {
            if incoming.is_empty() {
                break;
            }
            incoming = match &statement.node {
                Statement::Pass => incoming,
                Statement::Assignment(_)
                | Statement::CompoundAssignment(_)
                | Statement::FieldAssignment(_)
                | Statement::Expr(_) => self.region(statement.span, incoming),
                Statement::Return(value) => {
                    if let Some(value) = value {
                        let _ = self.region(value.span, incoming);
                    }
                    Vec::new()
                }
                Statement::Assert(assertion) => {
                    let AssertKind::Condition(condition) = &assertion.kind else {
                        return Err("operation execution requires an ordinary assertion");
                    };
                    let success = self.region(condition.span, incoming);
                    if let Some(message) = &assertion.message {
                        // Failure terminates after its message, including when either region produces no event.
                        let _ = self.region(message.span, success.clone());
                    }
                    success
                }
                Statement::If(branch) => {
                    let Condition::Expr(condition) = &branch.condition else {
                        return Err("operation paths require a checked expression condition");
                    };
                    let mut remaining = self.region(condition.span, incoming);
                    let mut outgoing = self.block(&branch.then_body, remaining.clone())?;
                    for (condition, body) in &branch.elif_branches {
                        remaining = self.region(condition.span, remaining);
                        outgoing.extend(self.block(body, remaining.clone())?);
                    }
                    outgoing.extend(match &branch.else_body {
                        Some(body) => self.block(body, remaining)?,
                        None => remaining,
                    });
                    outgoing
                }
                _ => return Err("operation execution requires the supported statement profile"),
            };
        }
        Ok(incoming)
    }

    /// Ordinary expressions execute their retained events sequentially. Empty literal regions preserve incoming
    /// predecessors, exactly as the source graph producer does; no synthetic evaluation node is invented.
    fn region(&mut self, span: HirSourceSpan, mut incoming: Vec<usize>) -> Vec<usize> {
        while self.graph.nodes.get(self.next).is_some_and(|node| {
            node.span.expansion == span.expansion && span.start <= node.span.start && node.span.end <= span.end
        }) {
            for predecessor in incoming {
                // Distinct empty arms can retain the same predecessor. Preserve the producer's edge multiplicity.
                self.expected[predecessor].push(self.next);
            }
            incoming = vec![self.next];
            self.next += 1;
        }
        incoming
    }
}
