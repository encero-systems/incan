//! End a lent local's storage right after its last use (#1604).
//!
//! A local passed to a callee at its last use used to move into that frame, which freed it when the callee returned.
//! Once the callee takes shared input the local is lent instead, and would live until its scope ends. The loan that
//! replaced such a move is marked where it is made (`retain_call_loans`); this pass drops the local straight after the
//! call, so lending never holds a value longer than the move it replaced.

use super::{IncanType, bir};

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

/// Drop each local after the call that lent it for the last time, and clear the marks.
///
/// The call's result must be unable to hold a loan of its argument, so only a call that stores a scalar or nothing
/// ends its argument here. Any other call leaves the local to its scope.
pub(super) fn drop_after_last_loans(statements: &mut Vec<bir::Statement>, locals: &[bir::LocalDecl]) {
    let mut index = 0;
    while index < statements.len() {
        let span = statements[index].span;
        let ended = ended_by(&mut statements[index], locals);
        nested(&mut statements[index], locals);
        for local in ended.into_iter().rev() {
            index += 1;
            statements.insert(
                index,
                bir::Statement {
                    kind: bir::StatementKind::Drop { local },
                    span,
                },
            );
        }
        index += 1;
    }
}

/// The marked loans of one call, cleared as they are read, when its result cannot borrow from them.
fn ended_by(statement: &mut bir::Statement, locals: &[bir::LocalDecl]) -> Vec<bir::LocalId> {
    let bir::StatementKind::Call { destination, args, .. } = &mut statement.kind else {
        return Vec::new();
    };
    let independent = destination.as_ref().is_none_or(|place| {
        place.projection.is_empty()
            && place
                .local_id()
                .and_then(|local| locals.get(local.index()))
                .is_some_and(|local| {
                    !matches!(local.ty, IncanType::Ref(_)) && local.ty.abi_v0_facts().ownership.is_trivially_copy()
                })
    });
    let mut ended = Vec::new();
    for argument in args {
        if let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument
            && read.fact == bir::OwnershipFact::Borrow
            && read.last_use
        {
            read.last_use = false;
            if let Some(local) = read.place.local_id()
                && independent
                && !ended.contains(&local)
            {
                ended.push(local);
            }
        }
    }
    ended
}

/// Visit every statement list inside one statement, closure and generator bodies included.
fn nested(statement: &mut bir::Statement, locals: &[bir::LocalDecl]) {
    match &mut statement.kind {
        bir::StatementKind::Assign { rvalue, .. } => match rvalue {
            bir::Rvalue::Match { arms, .. } => {
                for arm in arms {
                    drop_after_last_loans(&mut arm.guard_stmts, locals);
                    drop_after_last_loans(&mut arm.body_stmts, locals);
                }
            }
            bir::Rvalue::Closure { body, .. } => drop_after_last_loans(&mut body.stmts, locals),
            bir::Rvalue::Generator { body, .. } => drop_after_last_loans(&mut body.stmts, locals),
            _ => {}
        },
        bir::StatementKind::If {
            then_block, else_block, ..
        } => {
            drop_after_last_loans(&mut then_block.stmts, locals);
            if let Some(block) = else_block {
                drop_after_last_loans(&mut block.stmts, locals);
            }
        }
        bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => {
            drop_after_last_loans(&mut body.stmts, locals)
        }
        bir::StatementKind::Race { arms, .. } => {
            for arm in arms {
                drop_after_last_loans(&mut arm.body.stmts, locals);
            }
        }
        _ => {}
    }
}
