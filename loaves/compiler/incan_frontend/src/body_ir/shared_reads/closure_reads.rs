//! Read-only captures lent to an immediately invoked, non-escaping callback slot. Stored and escaping closures
//! retain their value-capture contract; lexical adjacency excludes intervening mutation or argument evaluation.

use std::collections::HashSet;

use super::{
    Candidate, FieldTypes, ReadContext, Signatures, bir, call_parameter, operand_type, overlapping, readonly,
    scalar_projection, uses, walk_mut, walk_neighbors,
};
use incan_lang::lang::surface::result_methods::ResultMethodId;

/// Recognize an immediate callback through canonical built-in semantics or a checked concrete only-called input.
fn immediate_callback(destination: &bir::Place, next: Option<&bir::Statement>, context: &ReadContext<'_>) -> bool {
    let Some(local) = destination.local_id() else {
        return false;
    };
    if !destination.projection.is_empty()
        || !context
            .locals
            .get(local.index())
            .is_some_and(|local| matches!(local.origin, bir::LocalOrigin::Temporary))
    {
        return false;
    }
    let Some(bir::Statement {
        kind: bir::StatementKind::Call { callee, args, .. },
        ..
    }) = next
    else {
        return false;
    };
    if args
        .iter()
        .filter(|argument| matches!(argument.as_one(), Some(bir::Operand::Place(read)) if read.place == *destination))
        .count()
        != 1
    {
        return false;
    }
    let Some(index) = args.iter().position(
        |argument| matches!(argument.as_one(), Some(bir::Operand::Place(read)) if read.place == *destination),
    ) else {
        return false;
    };
    let built_in = matches!(callee, bir::Callee::Method(target) if matches!(
        target.builtin_result(),
        Some(
            ResultMethodId::Map
                | ResultMethodId::MapErr
                | ResultMethodId::AndThen
                | ResultMethodId::OrElse
                | ResultMethodId::Inspect
                | ResultMethodId::InspectErr
        )
    ) && args.len() == 2 && index == 1);
    built_in
        || call_parameter(callee, index, context.signatures, context.candidates)
            .is_some_and(|parameter| parameter.callback_does_not_escape)
}

/// Do not introduce a loan while another input can change or transfer the same storage. Exclusive arguments are
/// conservatively excluded even through different roots: this pass does not establish alias identities for them.
fn independent_capture(
    operand: &bir::Operand,
    destination: &bir::Place,
    next: Option<&bir::Statement>,
    context: &ReadContext<'_>,
) -> bool {
    let bir::Operand::Place(capture) = operand else {
        return true;
    };
    let Some(bir::Statement {
        kind: bir::StatementKind::Call { callee, args, .. },
        ..
    }) = next
    else {
        return false;
    };
    args.iter().enumerate().all(|(index, argument)| {
        let Some(value) = argument.as_one() else { return false };
        let bir::Operand::Place(read) = value else { return true };
        if read.place == *destination {
            return true;
        }
        read.fact != bir::OwnershipFact::MutBorrow
            && !overlapping(&capture.place, &read.place)
            && !call_parameter(callee, index, context.signatures, context.candidates).is_some_and(|parameter| {
                matches!(parameter.ty, incan_semantics_core::IncanType::RefMut(_))
                    || parameter.access(context.candidates) == incan_semantics_core::IncanInputAccess::Exclusive
            })
    })
}

/// Prove a captured local's complete deferred body and result against the same shared-call hypothesis as its caller.
fn capture_is_read_only(body: &bir::ClosureBody, local: bir::LocalId, context: &ReadContext<'_>) -> bool {
    let captured = ReadContext {
        local,
        cursor: false,
        derived: false,
        ..*context
    };
    !body.consumes_environment
        && body.native_storage.is_none()
        && readonly(&body.stmts, &captured)
        && (!uses(&body.result, local) || scalar_projection(&body.result, local))
}

/// Let a parameter remain shared when its only capture is read during the immediately following callback call.
pub(super) fn shared_statement(
    statement: &bir::Statement,
    next: Option<&bir::Statement>,
    context: &ReadContext<'_>,
) -> bool {
    if let bir::StatementKind::Assign {
        place,
        rvalue: bir::Rvalue::Closure {
            captured_operands,
            body,
            ..
        },
    } = &statement.kind
        && immediate_callback(place, next, context)
        && captured_operands.len() == body.capture_locals.len()
    {
        return captured_operands
            .iter()
            .zip(&body.capture_locals)
            .all(|(operand, local)| {
                !uses(operand, context.local)
                    || (independent_capture(operand, place, next, context)
                        && capture_is_read_only(body, *local, context))
            });
    }
    super::shared_statement(statement, context)
}

/// Retain call-graph dependencies inside admitted immediate closures as well as the caller's lexical statements.
/// A callee losing shared access invalidates the enclosing parameter's proof through this same reverse graph.
pub(super) fn dependencies(statements: &[bir::Statement], context: &ReadContext<'_>) -> HashSet<Candidate> {
    frame_dependencies(statements, context, false)
}

/// A callback can pass a field through compiler temporaries before calling its reader. Track every candidate call
/// input inside that frame, not just operands still rooted at the capture. Extra edges only schedule a recheck;
/// omitting one could leave the enclosing parameter shared after its callback's callee becomes consuming.
fn frame_dependencies(statements: &[bir::Statement], context: &ReadContext<'_>, callback: bool) -> HashSet<Candidate> {
    let mut dependencies = HashSet::new();
    walk_neighbors(statements, &mut |statement, next| {
        if let bir::StatementKind::Call { callee, args, .. } = &statement.kind {
            for (index, argument) in args.iter().enumerate() {
                let Some(value) = argument.as_one() else { continue };
                if !callback && !uses(value, context.local) {
                    continue;
                }
                if let Some(dependency) = call_parameter(callee, index, context.signatures, context.candidates)
                    .filter(|selected| {
                        callback
                            || selected.shared_owner(context.candidates).is_some_and(|owner| {
                                operand_type(value, context.locals, context.field_types).as_ref() == Some(owner)
                            })
                    })
                    .and_then(|selected| selected.candidate)
                    .filter(|dependency| context.candidates.contains(dependency))
                {
                    dependencies.insert(dependency);
                }
            }
        }
        if let bir::StatementKind::Assign {
            place,
            rvalue:
                bir::Rvalue::Closure {
                    captured_operands,
                    body,
                    ..
                },
        } = &statement.kind
            && immediate_callback(place, next, context)
        {
            for (operand, local) in captured_operands.iter().zip(&body.capture_locals) {
                if uses(operand, context.local) {
                    let captured = ReadContext {
                        local: *local,
                        cursor: false,
                        derived: false,
                        ..*context
                    };
                    dependencies.extend(frame_dependencies(&body.stmts, &captured, true));
                }
            }
        }
    });
    dependencies
}

/// Publish capture loans only after callable signatures settle. The callee cannot retain the callback past the call,
/// and each loan's deferred body has independently proved that it neither changes nor transfers the captured owner.
pub(super) fn retain(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    // ---- Prove capture loans against final callable signatures ----
    let mut loans = HashSet::new();
    walk_neighbors(&body.block.stmts, &mut |statement, next| {
        let bir::StatementKind::Assign {
            place,
            rvalue:
                bir::Rvalue::Closure {
                    captured_operands,
                    body: closure,
                    ..
                },
        } = &statement.kind
        else {
            return;
        };
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local: bir::LocalId(0),
            cursor: false,
            derived: false,
        };
        if !immediate_callback(place, next, &context) || captured_operands.len() != closure.capture_locals.len() {
            return;
        }
        let Some(destination) = place.local_id() else { return };
        for (index, (operand, local)) in captured_operands.iter().zip(&closure.capture_locals).enumerate() {
            let bir::Operand::Place(read) = operand else { continue };
            if !matches!(read.fact, bir::OwnershipFact::Clone | bir::OwnershipFact::Move) {
                continue;
            }
            let context = ReadContext {
                locals: &body.locals,
                signatures,
                field_types,
                candidates,
                local: *local,
                cursor: false,
                derived: false,
            };
            if independent_capture(operand, place, next, &context) && capture_is_read_only(closure, *local, &context) {
                loans.insert((destination, index));
            }
        }
    });

    // ---- Publish only the individually proved capture facts ----
    walk_mut(&mut body.block.stmts, &mut |statement| {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Closure { captured_operands, .. },
        } = &mut statement.kind
            && let Some(destination) = place.local_id()
        {
            for (index, operand) in captured_operands.iter_mut().enumerate() {
                if loans.contains(&(destination, index))
                    && let bir::Operand::Place(read) = operand
                {
                    read.fact = bir::OwnershipFact::Borrow;
                    read.last_use = false;
                }
            }
        }
    });
}
