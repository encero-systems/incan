//! Provenance checks for values projected into compiler temporaries and for iterator items derived from them.

use super::*;

/// Follow each compiler temporary initialized from the candidate and every builtin iteration item it yields. Returning,
/// storing, mutating or capturing a derived value refuses the owner before its initial read is rewritten as a loan.
pub(super) fn derived_temporaries_are_read_only(statements: &[bir::Statement], context: &ReadContext<'_>) -> bool {
    let mut temporaries = Vec::new();
    walk(statements, &mut |statement| {
        let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(source),
        } = &statement.kind
        else {
            return;
        };
        let Some(source_type) = operand_type(source, context.locals, context.field_types) else {
            return;
        };
        if uses(source, context.local) && borrowed_temporary(source, place, &source_type, context.locals) {
            if let Some(local) = place.local_id() {
                let provisional_move = matches!(source,
                    bir::Operand::Place(read) if read.fact == bir::OwnershipFact::Move);
                temporaries.push((local, provisional_move));
            }
        }
    });
    temporaries.into_iter().all(|(temporary, provisional_move)| {
        derived_temporary_is_read_only(statements, temporary, provisional_move, context)
    })
}

/// Publish the reference carrier for compiler temporaries that observe a shared text parameter. The source parameter
/// remains the owner named by the source type. A temporary initialized by a checked borrow is already a loan; one
/// initialized by a move or clone becomes a loan only when its complete use is read-only, where reading a dictionary
/// entry by that key observes it. Every other temporary keeps its owned value, which lowering copies from the lent
/// text at that initialization.
pub(super) fn retain_shared_temporaries(
    body: &mut bir::Body,
    source: bir::LocalId,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    let mut retained = Vec::new();
    walk(&body.block.stmts, &mut |statement| {
        let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &statement.kind
        else {
            return;
        };
        if read.place.local_id() != Some(source) {
            return;
        }
        let Some(local) = place.local_id() else { return };
        let Some(declaration) = body.locals.get(local.index()) else {
            return;
        };
        if declaration.ty == IncanType::Primitive(IncanPrimitiveType::Str)
            && place.projection.is_empty()
            && read
                .place
                .projection
                .iter()
                .all(|step| matches!(step, bir::PlaceElem::Field { .. }))
            && matches!(declaration.origin, bir::LocalOrigin::Temporary)
            && body
                .locals
                .get(source.index())
                .is_some_and(|owner| owner.ty == declaration.ty)
            && matches!(
                read.fact,
                bir::OwnershipFact::Borrow | bir::OwnershipFact::Clone | bir::OwnershipFact::Move
            )
        {
            retained.push((local, declaration.ty.clone(), read.fact));
        }
    });
    retained.retain(|(local, _, fact)| {
        let context = ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local: *local,
            cursor: false,
            derived: true,
        };
        *fact == bir::OwnershipFact::Borrow || temporary_is_read_only(&body.block.stmts, &context, true)
    });
    for (local, owner, _) in &retained {
        if let Some(declaration) = body.locals.get_mut(local.index()) {
            declaration.ty = IncanType::Ref(Box::new(owner.clone()));
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
        if read.place.local_id() == Some(source)
            && place
                .local_id()
                .is_some_and(|local| retained.iter().any(|(retained, _, _)| *retained == local))
        {
            read.fact = bir::OwnershipFact::Borrow;
            read.last_use = false;
        }
    });
}

/// Prove a compiler temporary's complete use while exempting its one checked initialization. A caller may retain
/// that temporary as a reference only when every subsequent operation is already valid for a shared parameter.
/// With `lookup_keys`, reading a dictionary entry by the whole temporary is such an operation too.
pub(super) fn temporary_is_read_only(
    statements: &[bir::Statement],
    context: &ReadContext<'_>,
    lookup_keys: bool,
) -> bool {
    let mut initializations = 0;
    let mut accepted = true;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(_)),
        } if place.local_id() == Some(context.local) && place.projection.is_empty() => initializations += 1,
        bir::StatementKind::Assign { place, .. } if place.local_id() == Some(context.local) => accepted = false,
        bir::StatementKind::Drop { local } if *local == context.local => {}
        _ => accepted &= (lookup_keys && keyed_lookup(statement, context)) || shared_statement(statement, context),
    });
    accepted && initializations == 1
}

/// Admit a statement that reads the temporary only as the key of dictionary entry reads. A store's key, a slice bound
/// and every other operand position stay with the ordinary proof.
fn keyed_lookup(statement: &bir::Statement, context: &ReadContext<'_>) -> bool {
    let local = context.local;
    let unrelated = |value: &bir::Operand| !uses(value, local) && !projection_uses(value, local);
    let observed = |value: &bir::Operand| unrelated(value) || lookup_read(value, context);
    let kept = |place: &bir::Place| place.local_id() != Some(local) && !place_uses(place, local);
    match &statement.kind {
        bir::StatementKind::Return { value: Some(value) } | bir::StatementKind::Expr { value } => {
            lookup_read(value, context)
        }
        bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(value),
        } => kept(place) && lookup_read(value, context),
        bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Match { scrutinee, arms },
        } => {
            kept(place)
                && lookup_read(scrutinee, context)
                && arms
                    .iter()
                    .all(|arm| arm.guard.as_ref().is_none_or(unrelated) && unrelated(&arm.result))
        }
        bir::StatementKind::Call { destination, args, .. } => {
            destination.as_ref().is_none_or(kept)
                && args
                    .iter()
                    .any(|argument| argument.as_one().is_some_and(|value| lookup_read(value, context)))
                && args.iter().all(|argument| match argument {
                    bir::ArgumentElement::One(value) => observed(value),
                    _ => !argument_uses(argument, local),
                })
        }
        _ => false,
    }
}

/// A read rooted elsewhere whose every projection naming the temporary indexes a dictionary by the whole temporary.
fn lookup_read(value: &bir::Operand, context: &ReadContext<'_>) -> bool {
    let local = context.local;
    let bir::Operand::Place(read) = value else {
        return false;
    };
    let unrelated = |value: &bir::Operand| !uses(value, local) && !projection_uses(value, local);
    read.place.local_id() != Some(local)
        && place_uses(&read.place, local)
        && read
            .place
            .projection
            .iter()
            .enumerate()
            .all(|(depth, step)| match step {
                bir::PlaceElem::Index(index) if uses(index, local) => {
                    matches!(index.as_ref(), bir::Operand::Place(key) if key.place.projection.is_empty())
                        && dictionary_prefix(&read.place, depth, context)
                }
                bir::PlaceElem::Index(index) => unrelated(index),
                bir::PlaceElem::Slice { start, end, step } => {
                    [start, end, step].into_iter().flatten().all(|bound| unrelated(bound))
                }
                _ => true,
            })
}

/// Whether the projection prefix before `depth` selects a dictionary.
fn dictionary_prefix(place: &bir::Place, depth: usize, context: &ReadContext<'_>) -> bool {
    let prefix = bir::Place {
        root: place.root.clone(),
        projection: place.projection[..depth].to_vec(),
    };
    let Some(mut ty) = place_type(&prefix, context.locals, context.field_types) else {
        return false;
    };
    while let IncanType::Ref(inner) | IncanType::RefMut(inner) = ty {
        ty = *inner;
    }
    matches!(
        ty.collection(),
        Some((CollectionTypeId::Dict | CollectionTypeId::FrozenDict, _))
    )
}

/// Prove one derived temporary and every item it yields without counting the temporary initialization or item
/// production as uses of those derived locals themselves.
fn derived_temporary_is_read_only(
    statements: &[bir::Statement],
    temporary: bir::LocalId,
    provisional_move: bool,
    context: &ReadContext<'_>,
) -> bool {
    let temporary_context = ReadContext {
        locals: context.locals,
        signatures: context.signatures,
        field_types: context.field_types,
        candidates: context.candidates,
        local: temporary,
        cursor: false,
        derived: true,
    };
    let mut accepted = true;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(source),
        } if place.local_id() == Some(temporary) && uses(source, context.local) => {}
        bir::StatementKind::Assign { place, .. } if place.local_id() == Some(temporary) => accepted = false,
        bir::StatementKind::IterNext {
            destination,
            iterator: source,
            ..
        } if uses(source, temporary) => {
            accepted &= iteration_item_is_read_only(statements, destination.local_id(), context);
        }
        bir::StatementKind::Drop { local } if *local == temporary => {}
        bir::StatementKind::Call { callee, args, .. }
            if provisional_move && args.iter().any(|argument| argument_uses(argument, temporary)) =>
        {
            accepted &= read_only_callee(callee)
                && args
                    .iter()
                    .filter_map(bir::ArgumentElement::as_one)
                    .all(|value| !uses(value, temporary) || !through_payload(value));
        }
        _ => accepted &= shared_statement(statement, &temporary_context),
    });
    accepted
}

/// Prove uses of one builtin loop item while exempting the `IterNext` statement that initializes it.
fn iteration_item_is_read_only(
    statements: &[bir::Statement],
    item: Option<bir::LocalId>,
    context: &ReadContext<'_>,
) -> bool {
    let Some(item) = item else { return false };
    let item_context = ReadContext {
        locals: context.locals,
        signatures: context.signatures,
        field_types: context.field_types,
        candidates: context.candidates,
        local: item,
        cursor: false,
        derived: true,
    };
    let mut accepted = true;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::IterNext { destination, .. } if destination.local_id() == Some(item) => {}
        _ => accepted &= shared_statement(statement, &item_context),
    });
    accepted
}
