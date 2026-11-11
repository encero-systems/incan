use super::*;

/// Retain a compiler-created string argument as a reference when its source and complete use are observations. This
/// closes the representation gap between an inferred-shared callee and the expression temporary prepared by its
/// caller without granting user bindings implicit aliasing.
pub(super) fn retain_shared_call_temporaries(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    let mut retained = Vec::new();
    for (local, call_count) in shared_string_call_counts(&body.block.stmts, signatures, candidates) {
        if call_count != 1 {
            continue;
        }
        let Some(declaration) = body.locals.get(local.index()) else {
            continue;
        };
        if declaration.ty != IncanType::Primitive(IncanPrimitiveType::Str)
            || !matches!(declaration.origin, bir::LocalOrigin::Temporary)
        {
            continue;
        }
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local,
            cursor: false,
            derived: true,
        };
        if temporary_is_read_only(&body.block.stmts, &context)
            && has_retainable_string_initializer(&body.block.stmts, &body.locals, local)
        {
            retained.push(local);
        }
    }
    for local in &retained {
        if let Some(declaration) = body.locals.get_mut(local.index()) {
            declaration.ty = IncanType::Ref(Box::new(IncanType::Primitive(IncanPrimitiveType::Str)));
        }
    }
    walk_mut(&mut body.block.stmts, &mut |statement| {
        let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &mut statement.kind
        else {
            return;
        };
        if place.local_id().is_some_and(|local| retained.contains(&local)) {
            read.fact = bir::OwnershipFact::Borrow;
            read.last_use = false;
        }
    });
}

/// Require the temporary's owned string to come from storage that lowering can lend directly. Index, slice and
/// narrowed-payload reads materialize values rather than stable string places, so their temporary must stay owned
/// even when its only subsequent use is a shared call.
fn has_retainable_string_initializer(
    statements: &[bir::Statement],
    locals: &[bir::LocalDecl],
    local: bir::LocalId,
) -> bool {
    let mut initializers = 0;
    let mut retainable = true;
    let mut source = None;
    let mut live = false;
    let mut consumed = false;
    walk(statements, &mut |statement| {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &statement.kind
            && place.local_id() == Some(local)
            && place.projection.is_empty()
        {
            initializers += 1;
            retainable &= read
                .place
                .projection
                .iter()
                .all(|step| matches!(step, bir::PlaceElem::Field { .. }));
            source = Some(read.place.clone());
            live = true;
            return;
        }
        if !live || consumed {
            return;
        }
        if matches!(&statement.kind, bir::StatementKind::Call { args, .. }
            if args.iter().any(|argument| argument_uses(argument, local)))
        {
            consumed = true;
            return;
        }
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            ..
        } = &statement.kind
            && read.fact == bir::OwnershipFact::Move
            && source.as_ref().is_some_and(|source| overlapping(source, &read.place))
        {
            retainable = false;
        }
    });
    let stable_source = source
        .as_ref()
        .and_then(bir::Place::local_id)
        .and_then(|source| locals.get(source.index()))
        .is_some_and(|source| !matches!(source.origin, bir::LocalOrigin::Temporary));
    retainable && consumed && initializers == 1 && stable_source
}

/// Count selected shared-string calls by whole compiler temporary in one body walk, so bodies with many unrelated
/// string temporaries do not turn the caller-retention pass into a locals-by-statements scan.
fn shared_string_call_counts(
    statements: &[bir::Statement],
    signatures: &Signatures,
    candidates: &HashSet<Candidate>,
) -> HashMap<bir::LocalId, usize> {
    let mut counts = HashMap::new();
    walk(statements, &mut |statement| {
        let bir::StatementKind::Call { callee, args, .. } = &statement.kind else {
            return;
        };
        for (index, argument) in args.iter().enumerate() {
            let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument else {
                continue;
            };
            let Some(local) = read.place.local_id().filter(|_| read.place.projection.is_empty()) else {
                continue;
            };
            if call_parameter(callee, index, signatures, candidates)
                .and_then(|parameter| parameter.shared_owner(candidates))
                == Some(&IncanType::Primitive(IncanPrimitiveType::Str))
            {
                *counts.entry(local).or_insert(0) += 1;
            }
        }
    });
    counts
}
