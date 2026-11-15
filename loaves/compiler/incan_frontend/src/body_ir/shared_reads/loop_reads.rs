//! Loop items lent to their loop: a builtin list loop whose body only reads its item binds a reference to each element.

use super::match_reads::{formats_reference, lend_locals, lendable_value};
use super::*;

/// Bind a plain `for` item by reference to its element when the loop only reads it (#1605), instead of copying each
/// element out of the list. The loop polls a builtin iterator that lends a list, its item is text or a model or enum,
/// and its body only reads both the item and the list's root local, so every element stays in place while its
/// reference lives. The item's local becomes a reference to the element type, and lowering binds it through the
/// list's checked element loan. This runs on final signatures, so no callable's input changes.
pub(super) fn retain_borrowed_loop_items(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    // An open element type has no settled shared form, and a loan must not be held across a suspension.
    if !body.type_parameters.is_empty()
        || !body.nominal_instantiation.is_empty()
        || body.trait_default_instantiation.is_some()
        || body.is_async
        || body.is_generator()
    {
        return;
    }
    let lent_lists = lent_list_iterators(&body.block.stmts, &body.locals, field_types);
    if lent_lists.is_empty() {
        return;
    }
    let mut lent = Vec::new();
    walk(&body.block.stmts, &mut |statement| {
        let bir::StatementKind::Loop { body: block } = &statement.kind else {
            return;
        };
        let Some((poll, rest)) = block.stmts.split_first() else {
            return;
        };
        let bir::StatementKind::IterNext {
            destination,
            iterator: bir::Operand::Place(cursor),
            protocol: bir::IterProtocol::Builtin,
        } = &poll.kind
        else {
            return;
        };
        let (Some(item), Some(iterator)) = (destination.local_id(), cursor.place.local_id()) else {
            return;
        };
        let Some((root, element)) = lent_lists.get(&iterator) else {
            return;
        };
        let lendable = destination.projection.is_empty()
            && cursor.place.projection.is_empty()
            && body.locals.get(item.index()).is_some_and(|local| {
                local.origin == bir::LocalOrigin::UserBinding
                    && local.ty == *element
                    && lendable_value(&local.ty, &body.named_type_identities)
            });
        let context = |local| ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local,
            cursor: false,
            derived: false,
        };
        if lendable
            && readonly(rest, &context(item))
            && readonly(rest, &context(*root))
            && !formats_reference(rest, item, &body.locals)
        {
            lent.push(item);
        }
    });
    lend_locals(body, &lent, signatures, field_types, candidates);
}

/// The temporaries initialized by lending a list, each with the list's root local and element type. A builtin loop's
/// iterator is one of them when the loop neither takes nor changes its items.
fn lent_list_iterators(
    statements: &[bir::Statement],
    locals: &[bir::LocalDecl],
    field_types: &FieldTypes,
) -> HashMap<bir::LocalId, (bir::LocalId, IncanType)> {
    let mut iterators = HashMap::new();
    walk(statements, &mut |statement| {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
        } = &statement.kind
            && place.projection.is_empty()
            && read.fact == bir::OwnershipFact::Borrow
            && let (Some(iterator), Some(root)) = (place.local_id(), read.place.local_id())
            && locals
                .get(iterator.index())
                .is_some_and(|local| local.origin == bir::LocalOrigin::Temporary)
            && let Some(element) = place_type(&read.place, locals, field_types).and_then(|ty| {
                ty.collection_arguments(CollectionTypeId::List)
                    .and_then(|arguments| arguments.first().cloned())
            })
        {
            iterators.insert(iterator, (root, element));
        }
    });
    iterators
}
