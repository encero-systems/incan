//! Keep the by-value input of a parameter its body consumes on any path. A caller that passes such a value at its
//! last use moves it into the frame, so lending it would only turn that caller's free transfer into a copy inside the
//! callee, on the paths that consume it. Only a parameter the body never consumes is shared: a caller then copies
//! nothing for it, even when it keeps its value. A published callable follows the same rule, and also keeps each input
//! the base's read-only rule lends (`published_reads`), so no published input that was shared before becomes owned.

use crate::body_ir::ContextModules;
use std::collections::{HashMap, HashSet, VecDeque};

use super::{
    Candidate, FieldTypes, ReadContext, Signatures, bir, closure_reads, collect_signatures, pattern_is_read_only, walk,
    walk_neighbors,
};

/// Refuse shared input to every candidate its body consumes on any path, to a decreasing fixed point. A callee that
/// loses shared input turns its callers' forwarding moves into consumptions, so those callers are proved again.
pub(super) fn refuse_consumed_parameters(
    modules: &[bir::BodyIrModule],
    context: ContextModules<'_>,
    field_types: &FieldTypes,
    kept: &HashSet<Candidate>,
    candidates: &mut HashSet<Candidate>,
) {
    let signatures = collect_signatures(modules, context, candidates);
    let mut dependents: HashMap<Candidate, HashSet<Candidate>> = HashMap::new();
    let mut refused = VecDeque::new();
    for &candidate in candidates.iter() {
        if consumed(modules, candidate, &signatures, field_types, kept, candidates) {
            refused.push_back(candidate);
        }
        for dependency in forwarded_inputs(modules, candidate, &signatures, field_types, candidates) {
            dependents.entry(dependency).or_default().insert(candidate);
        }
    }
    while let Some(refused_candidate) = refused.pop_front() {
        if !candidates.remove(&refused_candidate) {
            continue;
        }
        let Some(affected) = dependents.get(&refused_candidate) else {
            continue;
        };
        for &candidate in affected {
            if candidates.contains(&candidate)
                && !refused.contains(&candidate)
                && consumed(modules, candidate, &signatures, field_types, kept, candidates)
            {
                refused.push_back(candidate);
            }
        }
    }
}

/// Resolve one candidate and prove whether its body consumes it under the current hypothesis. An input in `kept`, which
/// the base's read-only rule lends to a published callable, is never refused.
fn consumed(
    modules: &[bir::BodyIrModule],
    candidate: Candidate,
    signatures: &Signatures,
    field_types: &FieldTypes,
    kept: &HashSet<Candidate>,
    candidates: &HashSet<Candidate>,
) -> bool {
    let (module_index, body_index, parameter_index) = candidate;
    if kept.contains(&candidate) {
        return false;
    }
    let Some(body) = modules
        .get(module_index)
        .and_then(|module| module.bodies.get(body_index))
    else {
        return false;
    };
    let Some(parameter) = body.params.get(parameter_index) else {
        return false;
    };
    let context = ReadContext {
        locals: &body.locals,
        signatures,
        field_types,
        candidates,
        local: parameter.local,
        cursor: false,
        derived: false,
    };
    consumed_on_any_path(&body.block.stmts, &context)
}

/// Whether the frame moves the parameter, or a text temporary the parameter moved into, to an owned use on any path:
/// a branch, a loop body and a match arm count as much as the frame's own statements.
fn consumed_on_any_path(statements: &[bir::Statement], context: &ReadContext<'_>) -> bool {
    let mut owners = vec![context.local];
    walk(statements, &mut |statement| {
        if let Some(temporary) = moved_temporary(statement, context.local, context.locals) {
            owners.push(temporary);
        }
    });
    let mut consumed = false;
    walk_neighbors(statements, &mut |statement, next| {
        consumed |= owners.iter().any(|&owner| {
            let owner_context = ReadContext {
                local: owner,
                derived: owner != context.local,
                ..*context
            };
            (moves(statement, owner) && !closure_reads::shared_statement(statement, next, &owner_context))
                || binds_payload(statement, owner)
        });
    });
    consumed
}

/// Whether this statement's own operands, not those of a nested block, transfer the whole local or one of its fields.
fn moves(statement: &bir::Statement, local: bir::LocalId) -> bool {
    let moved = |value: &bir::Operand| {
        matches!(value, bir::Operand::Place(read)
            if read.fact == bir::OwnershipFact::Move
                && read.place.local_id() == Some(local)
                && read.place.projection.iter().all(|step| matches!(step, bir::PlaceElem::Field { .. })))
    };
    let element = |argument: &bir::ArgumentElement| match argument {
        bir::ArgumentElement::One(value) | bir::ArgumentElement::Named { operand: value, .. } => moved(value),
        bir::ArgumentElement::Spread(spread) => moved(&spread.source),
    };
    match &statement.kind {
        bir::StatementKind::Assign { rvalue, .. } => match rvalue {
            bir::Rvalue::Use(value) | bir::Rvalue::UnaryOp(_, value) => moved(value),
            bir::Rvalue::Aggregate(_, elements) => elements.iter().any(element),
            bir::Rvalue::ResultVariant(variant) => moved(&variant.payload),
            bir::Rvalue::Closure { captured_operands, .. } => captured_operands.iter().any(moved),
            bir::Rvalue::Generator {
                source,
                captured_operands,
                ..
            } => moved(source) || captured_operands.iter().any(moved),
            bir::Rvalue::Dict(entries) => entries.iter().any(|entry| match entry {
                bir::DictEntry::Pair(key, value) => moved(key) || moved(value),
                bir::DictEntry::Spread(spread) => moved(&spread.source),
            }),
            bir::Rvalue::Match { scrutinee, .. } => moved(scrutinee),
            _ => false,
        },
        bir::StatementKind::Call { args, .. } => args.iter().any(element),
        bir::StatementKind::Return { value: Some(value) } => moved(value),
        _ => false,
    }
}

/// Whether a match or pattern test binds a payload of the whole local, or of one of its fields, by value: a moved,
/// cloned or exclusively borrowed binding takes storage out of the subject, which needs the owned subject. Lowering
/// cannot move a native enum's payload out of shared storage, and on the base such a match kept the subject owned.
fn binds_payload(statement: &bir::Statement, local: bir::LocalId) -> bool {
    let subject = |value: &bir::Operand| {
        matches!(value, bir::Operand::Place(read)
            if read.place.local_id() == Some(local)
                && read.place.projection.iter().all(|step| matches!(step, bir::PlaceElem::Field { .. })))
    };
    match &statement.kind {
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Match { scrutinee, arms },
            ..
        } => subject(scrutinee) && arms.iter().any(|arm| !pattern_is_read_only(&arm.pattern)),
        bir::StatementKind::Assert {
            kind: bir::AssertionKind::Pattern { scrutinee, pattern },
            ..
        } => subject(scrutinee) && !pattern_is_read_only(pattern),
        _ => false,
    }
}

/// A compiler temporary that takes the whole parameter by a move. Text is the one carrier whose move into a
/// temporary the shared proof admits, so the temporary's own later transfer consumes the parameter.
fn moved_temporary(
    statement: &bir::Statement,
    parameter: bir::LocalId,
    locals: &[bir::LocalDecl],
) -> Option<bir::LocalId> {
    let bir::StatementKind::Assign {
        place,
        rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
    } = &statement.kind
    else {
        return None;
    };
    if read.fact != bir::OwnershipFact::Move
        || read.place.local_id() != Some(parameter)
        || !read.place.projection.is_empty()
        || !place.projection.is_empty()
    {
        return None;
    }
    place.local_id().filter(|local| {
        matches!(
            locals.get(local.index()).map(|local| &local.origin),
            Some(bir::LocalOrigin::Temporary)
        )
    })
}

/// The candidate inputs this parameter, or a temporary it moved into, is passed to. Losing one of them can turn that
/// call into a consumption, so each is a dependency of this candidate's proof.
fn forwarded_inputs(
    modules: &[bir::BodyIrModule],
    candidate: Candidate,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) -> HashSet<Candidate> {
    let (module_index, body_index, parameter_index) = candidate;
    let Some(body) = modules
        .get(module_index)
        .and_then(|module| module.bodies.get(body_index))
    else {
        return HashSet::new();
    };
    let Some(parameter) = body.params.get(parameter_index) else {
        return HashSet::new();
    };
    let mut owners = vec![parameter.local];
    walk(&body.block.stmts, &mut |statement| {
        if let Some(temporary) = moved_temporary(statement, parameter.local, &body.locals) {
            owners.push(temporary);
        }
    });
    let mut dependencies = HashSet::new();
    for owner in owners {
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local: owner,
            cursor: false,
            derived: owner != parameter.local,
        };
        dependencies.extend(closure_reads::dependencies(&body.block.stmts, &context));
    }
    dependencies
}
