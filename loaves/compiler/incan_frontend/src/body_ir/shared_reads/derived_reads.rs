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
                temporaries.push(local);
            }
        }
    });
    temporaries
        .into_iter()
        .all(|temporary| derived_temporary_is_read_only(statements, temporary, context))
}

/// Prove one derived temporary and every item it yields without counting the temporary initialization or item
/// production as uses of those derived locals themselves.
fn derived_temporary_is_read_only(
    statements: &[bir::Statement],
    temporary: bir::LocalId,
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
        bir::StatementKind::Assign { place, .. } if place.local_id() == Some(temporary) => {}
        bir::StatementKind::IterNext {
            destination,
            iterator: source,
            ..
        } if uses(source, temporary) => {
            accepted &= iteration_item_is_read_only(statements, destination.local_id(), context);
        }
        bir::StatementKind::Drop { local } if *local == temporary => {}
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
