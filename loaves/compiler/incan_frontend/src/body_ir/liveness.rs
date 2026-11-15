//! Backward liveness over one finished body (#1604).
//!
//! A whole-owner read that copies its owner transfers it instead when no statement on any path after the read reads
//! that owner again. The analysis walks the structured statements backward, joins the paths of an `If`, a `Match` and
//! a race, and iterates a `Loop` to a fixed point, so a value read once on each branch transfers where the textual
//! countdown copied it. A read inside a loop keeps its copy: lowering requires every owner a loop enters with to hold
//! its value whenever the loop repeats or exits. It only ever turns a copy into a transfer: every fact the builder
//! already selected stays as it is.

use super::*;

/// Locals some statement on some later path still reads.
type Live = HashSet<bir::LocalId>;

/// Live sets a `Break` and a `Continue` of one enclosing loop continue with.
struct LoopExits {
    after: Live,
    head: Live,
}

/// One frame's analysis state: the function body itself, or one closure or generator body inside it.
struct Liveness<'a> {
    /// Owners the frame holds by value, whose copied whole read may transfer.
    owners: &'a HashSet<bir::LocalId>,
    /// Enclosing loops of the statement being visited, innermost last.
    loops: Vec<LoopExits>,
    /// Locals still read after the frame returns: a closure's captures, since it may run again.
    exit: Live,
    /// Whether facts are rewritten. A loop's fixed-point rounds only compute live sets.
    apply: bool,
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Transfer each owner at the read no later statement follows on any path, instead of copying it there.
    ///
    /// An owner is a non-Copy value this frame holds by value in a source binding, an owned parameter or a temporary:
    /// never a receiver, caller-owned parameter storage, a capture or an unresolved reference. Its scope-exit drop
    /// stays, and rustc elaborates it to the paths that still own the value (DD-0018).
    pub(super) fn transfer_at_last_reads(&self, statements: &mut [bir::Statement]) {
        let owners = self
            .locals
            .iter()
            .filter(|local| {
                matches!(
                    local.origin,
                    bir::LocalOrigin::UserBinding | bir::LocalOrigin::Parameter | bir::LocalOrigin::Temporary
                )
            })
            .filter(|local| !matches!(local.ty, IncanType::Ref(_)) && !self.reads_as_copy(&local.ty))
            .filter(|local| !self.is_borrowed_parameter(local.id))
            .map(|local| local.id)
            .collect::<HashSet<_>>();
        if owners.is_empty() {
            return;
        }
        let mut liveness = Liveness {
            owners: &owners,
            loops: Vec::new(),
            exit: Live::new(),
            apply: true,
        };
        let mut live = Live::new();
        liveness.block(statements, &mut live);
    }
}

impl Liveness<'_> {
    /// Visit a statement list backward. `live` enters as the set after the list and leaves as the set before it.
    fn block(&mut self, statements: &mut [bir::Statement], live: &mut Live) {
        for statement in statements.iter_mut().rev() {
            self.statement(statement, live);
        }
    }

    fn statement(&mut self, statement: &mut bir::Statement, live: &mut Live) {
        match &statement.kind {
            bir::StatementKind::Return { .. } => *live = self.exit.clone(),
            bir::StatementKind::Break { .. } => {
                if let Some(exits) = self.loops.last() {
                    *live = exits.after.clone();
                }
            }
            // An exhausted iterator leaves the loop here, so everything read after the loop is still read.
            bir::StatementKind::IterNext { .. } => {
                if let Some(exits) = self.loops.last() {
                    live.extend(exits.after.iter().copied());
                }
            }
            bir::StatementKind::Continue => {
                if let Some(exits) = self.loops.last() {
                    *live = exits.head.clone();
                }
            }
            _ => {}
        }
        self.write(&statement.kind, live);
        self.nested(&mut statement.kind, live);
        self.reads(&mut statement.kind, live);
    }

    /// A whole-local destination ends the value it replaces. A projected write keeps its owner read: the owner must
    /// still hold a value when the statement stores into it.
    fn write(&self, kind: &bir::StatementKind, live: &mut Live) {
        let destination = match kind {
            bir::StatementKind::Assign { place, .. }
            | bir::StatementKind::TryPropagate { destination: place, .. }
            | bir::StatementKind::IterNext { destination: place, .. } => Some(place),
            bir::StatementKind::Call { destination, .. }
            | bir::StatementKind::Await { destination, .. }
            | bir::StatementKind::Race { destination, .. } => destination.as_ref(),
            _ => None,
        };
        let Some(place) = destination else {
            return;
        };
        let Some(local) = place.local_id() else {
            read_projection(place, live);
            return;
        };
        if place.projection.is_empty() {
            live.remove(&local);
        } else {
            live.insert(local);
            read_projection(place, live);
        }
    }

    /// Visit the control flow a statement runs after its own operands, joining the paths it selects between.
    fn nested(&mut self, kind: &mut bir::StatementKind, live: &mut Live) {
        match kind {
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                let mut otherwise = live.clone();
                self.block(&mut then_block.stmts, live);
                if let Some(block) = else_block {
                    self.block(&mut block.stmts, &mut otherwise);
                }
                live.extend(otherwise);
            }
            bir::StatementKind::Loop { body } => self.repeated(body, live),
            bir::StatementKind::UnsafeRegion { body, .. } => self.block(&mut body.stmts, live),
            bir::StatementKind::Race { arms, .. } => {
                let after = std::mem::take(live);
                for arm in arms {
                    let mut selected = after.clone();
                    self.read(&mut arm.result, &mut selected, &Live::new());
                    self.block(&mut arm.body.stmts, &mut selected);
                    selected.remove(&arm.binding);
                    live.extend(selected);
                }
            }
            bir::StatementKind::Assign { rvalue, .. } => match rvalue {
                bir::Rvalue::Match { arms, .. } => self.arms(arms, live),
                bir::Rvalue::Closure { body, .. } => {
                    let exit = body.capture_locals.iter().copied().collect();
                    let mut frame = self.frame(exit);
                    let mut inner = frame.exit.clone();
                    frame.read(&mut body.result, &mut inner, &Live::new());
                    frame.block(&mut body.stmts, &mut inner);
                }
                bir::Rvalue::Generator { body, .. } => {
                    let exit = body.capture_locals.iter().copied().collect();
                    let mut frame = self.frame(exit);
                    let mut inner = frame.exit.clone();
                    frame.block(&mut body.stmts, &mut inner);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The independent frame of a closure or generator body, whose captures outlive each run of it.
    fn frame(&self, exit: Live) -> Liveness<'_> {
        Liveness {
            owners: self.owners,
            loops: Vec::new(),
            exit,
            apply: self.apply,
        }
    }

    /// Join a match's arms. A pattern or guard that fails continues with the next arm, so an arm's guard statements
    /// also keep every owner a later arm reads.
    fn arms(&mut self, arms: &mut [bir::MatchArm], live: &mut Live) {
        let after = std::mem::take(live);
        let mut following = Live::new();
        for arm in arms.iter_mut().rev() {
            let mut selected = after.clone();
            self.read(&mut arm.result, &mut selected, &Live::new());
            self.block(&mut arm.body_stmts, &mut selected);
            selected.extend(following.iter().copied());
            if let Some(guard) = &mut arm.guard {
                self.read(guard, &mut selected, &Live::new());
            }
            self.block(&mut arm.guard_stmts, &mut selected);
            live.extend(selected.iter().copied());
            following = selected;
        }
    }

    /// Iterate a loop body until the set live at its head stops growing, then rewrite its facts once from that set.
    /// A local the body binds without an assignment, such as a pattern binding, stays live around the loop: its reads
    /// inside the body keep the fact the builder gave them.
    fn repeated(&mut self, body: &mut bir::Block, live: &mut Live) {
        let after = live.clone();
        let apply = std::mem::replace(&mut self.apply, false);
        let mut head = Live::new();
        loop {
            let next = self.iteration(body, &after, &head);
            if next == head {
                break;
            }
            head = next;
        }
        self.apply = apply;
        if apply {
            self.iteration(body, &after, &head);
        }
        *live = head;
    }

    /// The set live before one iteration whose end continues at `head` and whose breaks continue at `after`.
    fn iteration(&mut self, body: &mut bir::Block, after: &Live, head: &Live) -> Live {
        self.loops.push(LoopExits {
            after: after.clone(),
            head: head.clone(),
        });
        let mut live = head.clone();
        self.block(&mut body.stmts, &mut live);
        self.loops.pop();
        live.extend(head.iter().copied());
        live
    }

    /// Visit a statement's own operands, last evaluated first.
    ///
    /// An owner another operand of the same statement borrows stays copied: the loan is still held when the statement
    /// runs. A local callable is read before its arguments.
    fn reads(&mut self, kind: &mut bir::StatementKind, live: &mut Live) {
        let callable = match kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
                ..
            } => Some(target.operand.place.clone()),
            _ => None,
        };
        let iterates = matches!(kind, bir::StatementKind::IterNext { .. });
        let mut operands = Vec::new();
        own_operands(kind, &mut operands);
        let mut lent = operands
            .iter()
            .filter_map(|operand| match &**operand {
                bir::Operand::Place(read)
                    if matches!(read.fact, bir::OwnershipFact::Borrow | bir::OwnershipFact::MutBorrow) =>
                {
                    read.place.local_id()
                }
                _ => None,
            })
            .collect::<Live>();
        lent.extend(callable.as_ref().and_then(bir::Place::local_id));
        for operand in operands.into_iter().rev() {
            if iterates {
                read_operand(operand, live);
            } else {
                self.read(operand, live, &lent);
            }
        }
        if let Some(place) = &callable {
            read_place(place, live);
        }
    }

    /// Visit one operand: transfer its owner when nothing later reads it and no loop encloses the read, then keep the
    /// owner live before it.
    fn read(&mut self, operand: &mut bir::Operand, live: &mut Live, lent: &Live) {
        let bir::Operand::Place(read) = operand else {
            return;
        };
        if let Some(local) = read.place.local_id()
            && read.place.projection.is_empty()
            && read.fact == bir::OwnershipFact::Clone
            && self.owners.contains(&local)
            && !live.contains(&local)
            && !lent.contains(&local)
            && self.apply
            && self.loops.is_empty()
        {
            read.fact = bir::OwnershipFact::Move;
            read.last_use = true;
        }
        read_place(&read.place, live);
    }
}

/// Keep an operand's root and the operands of its projection live.
fn read_operand(operand: &bir::Operand, live: &mut Live) {
    if let bir::Operand::Place(read) = operand {
        read_place(&read.place, live);
    }
}

fn read_place(place: &bir::Place, live: &mut Live) {
    if let Some(local) = place.local_id() {
        live.insert(local);
    }
    read_projection(place, live);
}

fn read_projection(place: &bir::Place, live: &mut Live) {
    for element in &place.projection {
        match element {
            bir::PlaceElem::Index(index) => read_operand(index, live),
            bir::PlaceElem::Slice { start, end, step } => {
                for bound in [start, end, step].into_iter().flatten() {
                    read_operand(bound, live);
                }
            }
            bir::PlaceElem::Field { .. }
            | bir::PlaceElem::UnionMember { .. }
            | bir::PlaceElem::OptionPayload { .. } => {}
        }
    }
}

/// Collect the operands a statement evaluates itself, in evaluation order, leaving nested statements to the walk.
pub(in crate::body_ir) fn own_operands<'statement>(
    kind: &'statement mut bir::StatementKind,
    operands: &mut Vec<&'statement mut bir::Operand>,
) {
    match kind {
        bir::StatementKind::Assign { rvalue, .. } => rvalue_operands(rvalue, operands),
        bir::StatementKind::Call { args, .. } => argument_operands(args, operands),
        bir::StatementKind::If { cond, .. } => operands.push(cond),
        bir::StatementKind::Break { value } | bir::StatementKind::Return { value } => operands.extend(value.as_mut()),
        bir::StatementKind::Await { awaited, .. } => operands.push(awaited),
        bir::StatementKind::Race { arms, .. } => operands.extend(arms.iter_mut().map(|arm| &mut arm.awaitable)),
        bir::StatementKind::Yield { value } | bir::StatementKind::Expr { value } => operands.push(value),
        bir::StatementKind::Assert { kind, message, .. } => {
            match kind {
                bir::AssertionKind::Condition { cond } => operands.push(cond),
                bir::AssertionKind::Pattern { scrutinee, .. } => operands.push(scrutinee),
                bir::AssertionKind::Raises { call, .. } => operands.push(call),
            }
            operands.extend(message.as_mut());
        }
        bir::StatementKind::TryPropagate { operand, .. } => operands.push(operand),
        bir::StatementKind::IterNext { iterator, .. } => operands.push(iterator),
        bir::StatementKind::UnsafeRegion { .. }
        | bir::StatementKind::Drop { .. }
        | bir::StatementKind::Loop { .. }
        | bir::StatementKind::Continue
        | bir::StatementKind::Unsupported { .. } => {}
    }
}

fn rvalue_operands<'statement>(rvalue: &'statement mut bir::Rvalue, operands: &mut Vec<&'statement mut bir::Operand>) {
    match rvalue {
        bir::Rvalue::Use(value) | bir::Rvalue::UnaryOp(_, value) => operands.push(value),
        bir::Rvalue::BinaryOp(_, left, right) => operands.extend([left, right]),
        bir::Rvalue::IsInstance(test) => operands.push(&mut test.value),
        bir::Rvalue::Aggregate(_, elements) => argument_operands(elements, operands),
        bir::Rvalue::Dict(entries) => {
            for entry in entries {
                match entry {
                    bir::DictEntry::Pair(key, value) => operands.extend([key, &mut **value]),
                    bir::DictEntry::Spread(spread) => operands.push(&mut spread.source),
                }
            }
        }
        bir::Rvalue::ResultVariant(variant) => operands.push(&mut variant.payload),
        bir::Rvalue::Format(parts) => {
            for part in parts {
                if let bir::FormatPart::Expr { operand, .. } = part {
                    operands.push(&mut **operand);
                }
            }
        }
        bir::Rvalue::Closure { captured_operands, .. } => operands.extend(captured_operands.iter_mut()),
        bir::Rvalue::Generator {
            source,
            captured_operands,
            ..
        } => {
            operands.push(source);
            operands.extend(captured_operands.iter_mut());
        }
        bir::Rvalue::Match { scrutinee, .. } => operands.push(scrutinee),
        bir::Rvalue::ValueEnumVariant(_) | bir::Rvalue::FieldlessEnumVariant(_) => {}
    }
}

fn argument_operands<'statement>(
    arguments: &'statement mut [bir::ArgumentElement],
    operands: &mut Vec<&'statement mut bir::Operand>,
) {
    for argument in arguments {
        match argument {
            bir::ArgumentElement::One(operand) | bir::ArgumentElement::Named { operand, .. } => operands.push(operand),
            bir::ArgumentElement::Spread(spread) => operands.push(&mut spread.source),
        }
    }
}
