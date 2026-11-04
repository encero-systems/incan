//! Authenticate execution edges from the admitted source statements, including lazy assertion failure messages.

use incan_semantics_core::HirSourceSpan;

use super::super::super::flow;
use crate::ast::{AssertKind, Condition, Expr, MatchBody, Spanned, Statement};
use crate::typechecker::ownership_contracts::CallableSource;

/// Source event regions retain their producer order, but only these reconstructed edges establish execution.
/// This validator does not infer ownership from spans; canonical access and loan validation owns that contract.
pub(super) fn execution_order(graph: &flow::Graph, function: CallableSource<'_>) -> Result<Vec<usize>, &'static str> {
    let entry = graph.nodes.first().ok_or("operation execution has no entry")?;
    if graph.entry != 0 || entry.span != graph.owner.declaration_span || !matches!(entry.event, flow::Event::Nothing) {
        return Err("operation execution has no canonical source entry");
    }
    let mut paths = SourcePaths {
        graph,
        expected: vec![Vec::new(); graph.nodes.len()],
        next: 1,
        loops: Vec::new(),
    };
    let _ = paths.block(function.body(), vec![0])?;
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
    /// Each enclosing loop's header node and the paths that leave it through `break`.
    loops: Vec<(usize, Vec<usize>)>,
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
                // Each arm starts from the evaluated scrutinee. A guard's false edge reaches the next arm, while its
                // true edge enters that arm's body; continuing bodies join after the exhaustive match.
                Statement::Expr(Spanned {
                    node: Expr::Match(scrutinee, arms),
                    ..
                }) => {
                    let evaluated = self.region(scrutinee.span, incoming);
                    let mut outgoing = Vec::new();
                    let mut remaining = evaluated;
                    for arm in arms {
                        let arm_input = if let Some(guard) = &arm.node.guard {
                            let guarded = self.region(guard.span, remaining);
                            remaining = guarded.clone();
                            guarded
                        } else {
                            remaining.clone()
                        };
                        outgoing.extend(match &arm.node.body {
                            MatchBody::Expr(value) => self.region(value.span, arm_input),
                            MatchBody::Block(body) => self.block(body, arm_input)?,
                        });
                    }
                    outgoing
                }
                Statement::Assignment(_)
                | Statement::TupleUnpack(_)
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
                // Loops follow the source graph's own shape: a header, the condition or the iterable's poll, the
                // body with its back edge to the header, and exits from the condition or poll and each `break`.
                Statement::While(loop_) => {
                    let Condition::Expr(condition) = &loop_.condition else {
                        return Err("operation paths require a checked expression condition");
                    };
                    let header = self.node(incoming)?;
                    let tested = self.region(condition.span, vec![header]);
                    self.repeat(header, &loop_.body, tested)?
                }
                Statement::Loop(loop_) => {
                    let header = self.node(incoming)?;
                    self.repeat(header, &loop_.body, Vec::new())?
                }
                Statement::For(loop_) => {
                    let evaluated = self.region(loop_.iter.span, incoming);
                    let header = self.node(evaluated)?;
                    let polled = self.region(loop_.iter.span, vec![header]);
                    self.repeat(header, &loop_.body, polled)?
                }
                Statement::Break(None) => {
                    self.loops
                        .last_mut()
                        .ok_or("operation path break has no enclosing loop")?
                        .1
                        .extend(incoming);
                    Vec::new()
                }
                Statement::Continue => {
                    let header = self
                        .loops
                        .last()
                        .ok_or("operation path continue has no enclosing loop")?
                        .0;
                    self.connect(&incoming, header);
                    Vec::new()
                }
                _ => return Err("operation execution requires the supported statement profile"),
            };
        }
        Ok(incoming)
    }

    /// Run a loop body from its entry paths, linking every completed iteration back to the header. The loop leaves
    /// through `exits` (its failed condition or exhausted poll) and through each `break` in its body.
    fn repeat(
        &mut self,
        header: usize,
        body: &[Spanned<Statement>],
        exits: Vec<usize>,
    ) -> Result<Vec<usize>, &'static str> {
        let entry = if exits.is_empty() { vec![header] } else { exits.clone() };
        self.loops.push((header, Vec::new()));
        let completed = self.block(body, entry)?;
        self.connect(&completed, header);
        let (_, breaks) = self.loops.pop().ok_or("operation path loop has no owner")?;
        Ok(exits.into_iter().chain(breaks).collect())
    }

    /// The one event node the source graph creates for a loop header, after every incoming path.
    fn node(&mut self, incoming: Vec<usize>) -> Result<usize, &'static str> {
        let node = self.next;
        if node >= self.graph.nodes.len() {
            return Err("operation path loop header has no source event");
        }
        self.connect(&incoming, node);
        self.next += 1;
        Ok(node)
    }

    /// Record each predecessor's edge to `next`, as the source graph builder appends it.
    fn connect(&mut self, incoming: &[usize], next: usize) {
        for predecessor in incoming {
            self.expected[*predecessor].push(next);
        }
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
