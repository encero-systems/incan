//! End a lent local's storage right after its last use (#1604).
//!
//! A local passed to a callee at its last use used to move into that frame, which freed it when the callee returned.
//! Once the callee takes shared input the local is lent instead, and would live until its scope ends. Each loan that
//! replaced such a move is reported where it is made (`retain_call_loans`); this pass drops the local straight after
//! that call, so lending never holds a value longer than the move it replaced.
//!
//! A call's result holds a loan of an argument only through a declared returned view or result dependency, which
//! names its one source input, so a loan to any other input ends when the call returns, whatever the call returns.
//! The local must also have no other holder still in use: a reference bound to it or to a part of it earlier (a match
//! payload, a loop item, a cursor, a view) that a later statement reads keeps the local to its scope.

use std::collections::{HashMap, HashSet};

use super::{Candidate, IncanType, Signatures, bir, call_parameter};
use crate::body_ir::liveness::own_operands;

/// Whether a call argument about to become a loan was its owner's transfer: the whole value of a source binding,
/// read for the last time.
pub(super) fn replaces_last_transfer(read: &bir::PlaceOperand, locals: &[bir::LocalDecl], owner: &IncanType) -> bool {
    read.fact == bir::OwnershipFact::Move
        && read.last_use
        && read.place.projection.is_empty()
        && read
            .place
            .local_id()
            .and_then(|local| locals.get(local.index()))
            .is_some_and(|local| local.origin == bir::LocalOrigin::UserBinding && local.ty == *owner)
}

/// One read of a local by a statement. `call_scoped` marks a loan that ends when its call returns.
struct Read {
    local: bir::LocalId,
    fact: bir::OwnershipFact,
    call_scoped: bool,
}

/// What one statement, or the tail of a match, closure or race, reads and writes, in source order.
#[derive(Default)]
struct Entry {
    reads: Vec<Read>,
    writes: Vec<bir::LocalId>,
    /// The local a drop statement ends.
    dropped: Option<bir::LocalId>,
}

/// The body flattened in source order, with the entry of each visited statement.
struct Flow<'a> {
    locals: &'a [bir::LocalDecl],
    signatures: &'a Signatures,
    candidates: &'a HashSet<Candidate>,
    entries: Vec<Entry>,
    /// The entry of each statement, by its position in the call walk.
    positions: HashMap<usize, usize>,
    /// Statements inside a closure or generator body, whose frame runs apart from the statements around it.
    deferred: HashSet<usize>,
    /// Statements whose call stores its result in the named local.
    destinations: HashMap<usize, bir::LocalId>,
    visited: usize,
}

/// Drop each local after the call that lent it for the last time. `ended` names each such call by its position in the
/// call walk (`walk_calls_mut`) and the local it lent.
pub(super) fn drop_after_last_loans(
    body: &mut bir::Body,
    signatures: &Signatures,
    candidates: &HashSet<Candidate>,
    ended: &[(usize, bir::LocalId)],
) {
    if ended.is_empty() {
        return;
    }
    let mut flow = Flow {
        locals: &body.locals,
        signatures,
        candidates,
        entries: Vec::new(),
        positions: HashMap::new(),
        deferred: HashSet::new(),
        destinations: HashMap::new(),
        visited: 0,
    };
    flow.block(&mut body.block.stmts, false);
    let mut accepted: HashMap<usize, Vec<bir::LocalId>> = HashMap::new();
    for &(statement, local) in ended {
        if flow.ends_with_call(statement, local) {
            let locals = accepted.entry(statement).or_default();
            if !locals.contains(&local) {
                locals.push(local);
            }
        }
    }
    if !accepted.is_empty() {
        insert_drops(&mut body.block.stmts, &accepted, &mut 0);
    }
}

impl Flow<'_> {
    /// Record a statement list in source order, numbering statements as the call walk does.
    fn block(&mut self, statements: &mut [bir::Statement], deferred: bool) {
        for statement in statements {
            let ordinal = self.visited;
            self.visited += 1;
            if deferred {
                self.deferred.insert(ordinal);
            }
            // Dropping a holder ends a value that may still read the local; the local's own drop reads nothing.
            if let bir::StatementKind::Drop { local } = statement.kind {
                self.entries.push(Entry {
                    dropped: Some(local),
                    ..Entry::default()
                });
                continue;
            }
            let (head, tail) = self.entries_of(&mut statement.kind, ordinal);
            self.positions.insert(ordinal, self.entries.len());
            self.entries.push(head);
            for (nested, callable) in nested_lists(&mut statement.kind) {
                self.block(nested, deferred || callable);
            }
            self.entries.extend(tail);
        }
    }

    /// A statement's own reads and writes, and for a match, closure or race the reads and the write that follow its
    /// nested statements.
    fn entries_of(&mut self, kind: &mut bir::StatementKind, ordinal: usize) -> (Entry, Option<Entry>) {
        let mut head = Entry::default();
        let mut after = Entry::default();
        let stored = destination(kind).and_then(bir::Place::local_id);
        match kind {
            bir::StatementKind::Call { callee, args, .. } => {
                if let bir::Callee::Function(bir::CallableTarget::Local(target)) = callee {
                    head.reads.extend(read_of(&target.operand, false));
                }
                for (index, argument) in args.iter().enumerate() {
                    let (operand, plain) = match argument {
                        bir::ArgumentElement::One(operand) => (operand, true),
                        bir::ArgumentElement::Named { operand, .. } => (operand, false),
                        bir::ArgumentElement::Spread(spread) => (&spread.source, false),
                    };
                    let bir::Operand::Place(read) = operand else {
                        continue;
                    };
                    let call_scoped = plain
                        && call_parameter(callee, index, self.signatures, self.candidates).is_some_and(|parameter| {
                            parameter.shared_owner(self.candidates).is_some() && !parameter.result_holds_loan
                        });
                    head.reads.extend(read_of(read, call_scoped));
                }
                if let Some(local) = stored {
                    self.destinations.insert(ordinal, local);
                }
                head.writes.extend(stored);
                return (head, None);
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { scrutinee, arms },
                ..
            } => {
                let mut borrows = false;
                for arm in arms.iter() {
                    borrows |= bindings(&arm.pattern, &mut head.writes);
                    after.reads.extend(operand_read(&arm.result));
                    after.reads.extend(arm.guard.as_ref().and_then(operand_read));
                }
                if let Some(mut read) = operand_read(scrutinee) {
                    if borrows {
                        read.fact = bir::OwnershipFact::Borrow;
                    }
                    head.reads.push(read);
                }
                after.writes.extend(stored);
                return (head, Some(after));
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => {
                head.writes.extend(body.capture_locals.iter().copied());
                after.reads.extend(operand_read(&body.result));
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Generator { body, .. },
                ..
            } => head.writes.extend(body.capture_locals.iter().copied()),
            bir::StatementKind::Race { arms, .. } => {
                for arm in arms.iter() {
                    head.writes.push(arm.binding);
                    after.reads.extend(operand_read(&arm.result));
                }
            }
            _ => {}
        }
        let mut operands = Vec::new();
        own_operands(kind, &mut operands);
        head.reads
            .extend(operands.into_iter().filter_map(|operand| operand_read(operand)));
        head.writes.extend(stored);
        after.writes.extend(stored);
        let tail = (!after.reads.is_empty()).then_some(after);
        (head, tail)
    }

    /// Whether the loan `statement` makes of `local` is the last use of its storage: the call is in this frame, its
    /// result is stored elsewhere, and no statement after it names the local or a holder of it.
    fn ends_with_call(&self, statement: usize, local: bir::LocalId) -> bool {
        let Some(&position) = self.positions.get(&statement) else {
            return false;
        };
        if self.deferred.contains(&statement) || self.destinations.get(&statement) == Some(&local) {
            return false;
        }
        // A later statement that names the local itself is on another path, or gives it a new value; either way its
        // storage stays to its scope.
        let holders = self.holders(local);
        !self.entries[position + 1..].iter().any(|entry| {
            entry.dropped.is_some_and(|dropped| holders.contains(&dropped))
                || entry
                    .reads
                    .iter()
                    .any(|read| read.local == local || holders.contains(&read.local))
                || entry
                    .writes
                    .iter()
                    .any(|&written| written == local || holders.contains(&written))
        })
    }

    /// Every local that may hold a loan of `local`'s storage: one written by a statement that borrows the local
    /// beyond a call, or that reads another holder without copying its value out.
    fn holders(&self, local: bir::LocalId) -> HashSet<bir::LocalId> {
        let mut holders = HashSet::new();
        loop {
            let before = holders.len();
            for entry in &self.entries {
                let held = entry.reads.iter().any(|read| {
                    !read.call_scoped
                        && if read.local == local {
                            matches!(
                                read.fact,
                                bir::OwnershipFact::Borrow
                                    | bir::OwnershipFact::MutBorrow
                                    | bir::OwnershipFact::Unknown
                            )
                        } else {
                            holders.contains(&read.local) && read.fact != bir::OwnershipFact::Clone
                        }
                });
                if held {
                    holders.extend(
                        entry
                            .writes
                            .iter()
                            .copied()
                            .filter(|&written| written != local && self.holds_storage(written)),
                    );
                }
            }
            if holders.len() == before {
                return holders;
            }
        }
    }

    /// Whether a local's type can hold a loan at all.
    fn holds_storage(&self, local: bir::LocalId) -> bool {
        self.locals
            .get(local.index())
            .is_none_or(|declaration| holds_loan(&declaration.ty))
    }
}

/// Whether a value of this type can hold a loan: a reference always can; a scalar, text, bytes, and a tuple or
/// builtin collection of such values own all they reach. Any other type may wrap a reference.
fn holds_loan(ty: &IncanType) -> bool {
    match ty {
        IncanType::Ref(_) | IncanType::RefMut(_) => true,
        IncanType::Primitive(_) => false,
        IncanType::Tuple(elements) => elements.iter().any(holds_loan),
        _ => match ty.collection() {
            Some((_, arguments)) => arguments.iter().any(holds_loan),
            None => !ty.abi_v0_facts().ownership.is_trivially_copy(),
        },
    }
}

/// Insert the accepted drops, numbering statements as `Flow::block` did, and remove a later drop of the same local
/// from the same list: every path that reaches it has already run the new one.
fn insert_drops(
    statements: &mut Vec<bir::Statement>,
    accepted: &HashMap<usize, Vec<bir::LocalId>>,
    visited: &mut usize,
) {
    let mut index = 0;
    while index < statements.len() {
        let ordinal = *visited;
        *visited += 1;
        for (nested, _) in nested_lists(&mut statements[index].kind) {
            insert_drops(nested, accepted, visited);
        }
        let span = statements[index].span;
        for &local in accepted.get(&ordinal).into_iter().flatten() {
            index += 1;
            statements.insert(
                index,
                bir::Statement {
                    kind: bir::StatementKind::Drop { local },
                    span,
                },
            );
            let mut later = index + 1;
            while later < statements.len() {
                if matches!(statements[later].kind, bir::StatementKind::Drop { local: dropped } if dropped == local) {
                    statements.remove(later);
                } else {
                    later += 1;
                }
            }
        }
        index += 1;
    }
}

/// The statement lists inside one statement, in the order the call walk enters them, each with whether it is a closure
/// or generator body.
fn nested_lists(kind: &mut bir::StatementKind) -> Vec<(&mut Vec<bir::Statement>, bool)> {
    match kind {
        bir::StatementKind::Assign { rvalue, .. } => match rvalue {
            bir::Rvalue::Match { arms, .. } => arms
                .iter_mut()
                .flat_map(|arm| [(&mut arm.guard_stmts, false), (&mut arm.body_stmts, false)])
                .collect(),
            bir::Rvalue::Closure { body, .. } => vec![(&mut body.stmts, true)],
            bir::Rvalue::Generator { body, .. } => vec![(&mut body.stmts, true)],
            _ => Vec::new(),
        },
        bir::StatementKind::If {
            then_block, else_block, ..
        } => {
            let mut lists = vec![(&mut then_block.stmts, false)];
            if let Some(block) = else_block {
                lists.push((&mut block.stmts, false));
            }
            lists
        }
        bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
            vec![(&mut body.stmts, false)]
        }
        bir::StatementKind::Race { arms, .. } => arms.iter_mut().map(|arm| (&mut arm.body.stmts, false)).collect(),
        _ => Vec::new(),
    }
}

/// The place a statement stores its result in.
fn destination(kind: &bir::StatementKind) -> Option<&bir::Place> {
    match kind {
        bir::StatementKind::Assign { place, .. }
        | bir::StatementKind::TryPropagate { destination: place, .. }
        | bir::StatementKind::IterNext { destination: place, .. } => Some(place),
        bir::StatementKind::Call { destination, .. }
        | bir::StatementKind::Await { destination, .. }
        | bir::StatementKind::Race { destination, .. } => destination.as_ref(),
        _ => None,
    }
}

/// Collect every local a pattern binds, and report whether any binding borrows its payload.
fn bindings(pattern: &bir::Pattern, bound: &mut Vec<bir::LocalId>) -> bool {
    match pattern {
        bir::Pattern::Var(binding) => {
            bound.push(binding.local);
            matches!(
                binding.fact,
                bir::OwnershipFact::Borrow | bir::OwnershipFact::MutBorrow | bir::OwnershipFact::Unknown
            )
        }
        bir::Pattern::Tuple(fields)
        | bir::Pattern::Or(fields)
        | bir::Pattern::Result { fields, .. }
        | bir::Pattern::Enum { fields, .. }
        | bir::Pattern::UnionMember { fields, .. } => fields
            .iter()
            .fold(false, |borrows, field| bindings(field, bound) | borrows),
        bir::Pattern::Struct { fields, .. } | bir::Pattern::Nominal { fields, .. } => fields
            .iter()
            .fold(false, |borrows, (_, field)| bindings(field, bound) | borrows),
        bir::Pattern::Wildcard | bir::Pattern::Literal(_) | bir::Pattern::FieldlessEnumVariant(_) => false,
    }
}

/// The read an operand makes of a local's storage, if it reads one.
fn operand_read(operand: &bir::Operand) -> Option<Read> {
    match operand {
        bir::Operand::Place(read) => read_of(read, false),
        _ => None,
    }
}

/// One place read as an entry's read of its root local.
fn read_of(read: &bir::PlaceOperand, call_scoped: bool) -> Option<Read> {
    Some(Read {
        local: read.place.local_id()?,
        fact: read.fact,
        call_scoped,
    })
}
