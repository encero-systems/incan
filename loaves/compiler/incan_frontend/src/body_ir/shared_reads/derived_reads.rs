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

/// Publish the reference carrier for compiler temporaries whose complete use was admitted by the candidate proof.
/// The source parameter remains the owner named by the source type; only the compiler-created observation local and
/// its initializer become a loan.
pub(super) fn retain_shared_temporaries(body: &mut bir::Body, source: bir::LocalId) {
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
            retained.push((local, declaration.ty.clone()));
        }
    });
    for (local, owner) in &retained {
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
                .is_some_and(|local| retained.iter().any(|(retained, _)| *retained == local))
        {
            read.fact = bir::OwnershipFact::Borrow;
            read.last_use = false;
        }
    });
}

/// Prove a compiler temporary's complete use while exempting its one checked initialization. A caller may retain
/// that temporary as a reference only when every subsequent operation is already valid for a shared parameter.
pub(super) fn temporary_is_read_only(statements: &[bir::Statement], context: &ReadContext<'_>) -> bool {
    let mut initializations = 0;
    let mut accepted = true;
    walk(statements, &mut |statement| match &statement.kind {
        bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(_)),
        } if place.local_id() == Some(context.local) && place.projection.is_empty() => initializations += 1,
        bir::StatementKind::Assign { place, .. } if place.local_id() == Some(context.local) => accepted = false,
        bir::StatementKind::Drop { local } if *local == context.local => {}
        _ => accepted &= shared_statement(statement, context),
    });
    accepted && initializations == 1
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
