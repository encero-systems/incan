//! A generator expression's later `for` clauses, each of which a step expands through a nested chain of its own.

use incan_semantics_core::body_ir::{Block, Operand as SourceOperand, Place as SourcePlace, Rvalue, StatementKind};

use crate::lists::is_builtin_iteration;

/// One later clause: its iterator local, its source local, and the locals bound before it.
pub type LaterClause = (i64, i64, Vec<i64>);

/// Find each later `for` clause of one step body: its iterator local, its source local, and the locals bound before
/// it.
///
/// The step body follows its item's poll. Clauses nest, so a later clause ends its step, directly or under the step's
/// filters, and its loop yields; the locals bound on that path are what the clause's own step may read besides its
/// item. A loop that never yields, such as a comprehension inside the element, is not a clause.
pub fn later_clauses(block: &Block) -> Vec<LaterClause> {
    let mut clauses = Vec::new();
    visit_block(block, 1, &[], &mut clauses);
    clauses
}

/// Read the whole iterator local a loop polls first, or -1.
pub fn clause_iterator(body: &Block) -> i64 {
    match body.stmts.first() {
        Some(statement) => builtin_poll(&statement.kind).0,
        None => -1,
    }
}

/// Read the whole local a loop's first poll fills, or -1.
pub fn clause_item(body: &Block) -> i64 {
    match body.stmts.first() {
        Some(statement) => builtin_poll(&statement.kind).1,
        None => -1,
    }
}

/// Walk one block in order, recording each clause with the locals bound before it, and descend into filters.
fn visit_block(block: &Block, start: usize, bound: &[i64], clauses: &mut Vec<LaterClause>) {
    let mut assigned = bound.to_vec();
    let mut iterator = -1;
    let mut source = -1;
    for statement in block.stmts.iter().skip(start) {
        if iterator >= 0 && opens_clause(&statement.kind, iterator) {
            clauses.push((iterator, source, assigned.clone()));
        }
        visit_filters(&statement.kind, &assigned, clauses);
        (iterator, source) = iterator_initialization(&statement.kind);
        let local = assigned_local(&statement.kind);
        if local >= 0 && !assigned.contains(&local) {
            assigned.push(local);
        }
    }
}

/// Descend into a filter's branches with the locals bound so far.
fn visit_filters(kind: &StatementKind, bound: &[i64], clauses: &mut Vec<LaterClause>) {
    if let StatementKind::If {
        then_block, else_block, ..
    } = kind
    {
        visit_block(then_block, 0, bound, clauses);
        if let Some(nested) = else_block {
            visit_block(nested, 0, bound, clauses);
        }
    }
}

/// Recognize a loop that polls the iterator the previous statement initialized and yields.
fn opens_clause(kind: &StatementKind, iterator: i64) -> bool {
    match kind {
        StatementKind::Loop { body } => clause_iterator(body) == iterator && yields(body),
        _ => false,
    }
}

/// Report whether a block yields, directly or in a nested filter or loop.
fn yields(block: &Block) -> bool {
    block.stmts.iter().any(|statement| statement_yields(&statement.kind))
}

/// Find a yield in one statement or the blocks it nests.
fn statement_yields(kind: &StatementKind) -> bool {
    match kind {
        StatementKind::Yield { .. } => true,
        StatementKind::Loop { body } => yields(body),
        StatementKind::If {
            then_block, else_block, ..
        } => yields(then_block) || else_block.as_ref().is_some_and(yields),
        _ => false,
    }
}

/// Read a builtin poll's whole iterator and destination locals, or (-1, -1).
fn builtin_poll(kind: &StatementKind) -> (i64, i64) {
    match kind {
        StatementKind::IterNext {
            destination,
            iterator: SourceOperand::Place(read),
            protocol,
        } if is_builtin_iteration(protocol) => (whole_local(&read.place), whole_local(destination)),
        _ => (-1, -1),
    }
}

/// Read an iterator's initialization from a whole source local, or (-1, -1).
fn iterator_initialization(kind: &StatementKind) -> (i64, i64) {
    match kind {
        StatementKind::Assign {
            place,
            rvalue: Rvalue::Use(SourceOperand::Place(read)),
        } => (whole_local(place), whole_local(&read.place)),
        _ => (-1, -1),
    }
}

/// Read the whole local an assignment or a call fills, or -1.
fn assigned_local(kind: &StatementKind) -> i64 {
    match kind {
        StatementKind::Assign { place, .. } => whole_local(place),
        StatementKind::Call {
            destination: Some(place),
            ..
        } => whole_local(place),
        _ => -1,
    }
}

/// Read a whole Body IR local's index, or -1 for a projection or a global.
pub fn whole_local(place: &SourcePlace) -> i64 {
    if !place.projection.is_empty() {
        return -1;
    }
    match place.local_id() {
        Some(local) => local.index() as i64,
        None => -1,
    }
}
