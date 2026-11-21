//! Loop items lent to their loop: a builtin list loop whose body only reads its item binds a reference to each element.

use super::match_reads::{formats_reference, lend_locals, lendable_value};
use super::*;

/// Bind a plain `for` item by reference to its element when the loop only reads it (#1605), instead of copying each
/// element out of the list. The loop polls a builtin iterator that lends a list, its item has a checked shared nominal
/// or text carrier, and its body only reads both the item and the list's root local, so every element stays in place
/// while its reference lives. The item's local becomes a reference to the element type, and lowering binds it through
/// the list's checked element loan. This runs on final signatures, so no callable's input changes.
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
    let lent_lists = lent_list_iterators(body, field_types);
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
        let native = registered_item(element, body, field_types);
        let lendable = destination.projection.is_empty()
            && cursor.place.projection.is_empty()
            && body.locals.get(item.index()).is_some_and(|local| {
                local.origin == bir::LocalOrigin::UserBinding
                    && local.ty == *element
                    && (native || lendable_value(&local.ty, &body.named_type_identities))
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
        let reads = |local, owned_return| {
            if native {
                native_loop_reads(rest, &context(local), owned_return)
            } else {
                readonly(rest, &context(local))
            }
        };
        if lendable
            && reads(item, body.returned_view.is_none().then_some(&body.return_type))
            && reads(*root, None)
            && !formats_reference(rest, item, &body.locals)
        {
            lent.push(item);
        }
    });
    lend_locals(body, &lent, signatures, field_types, candidates);
}

/// Require both the frame's exact Rust binding and an available closed Model or Enum layout for its reference.
fn registered_item(ty: &IncanType, body: &bir::Body, fields: &FieldTypes) -> bool {
    let IncanType::Named(name) = ty else { return false };
    let Some(owner) = body.named_type_identities.get(name) else {
        return false;
    };
    let incan_semantics_core::SymbolOrigin::RustCrate(path) = &owner.origin else {
        return false;
    };
    name.split("::").eq(path.iter().map(String::as_str))
        && fields.native_loop_items.get(owner).is_some_and(Option::is_some)
}

/// A shared Rust receiver can have interior effects. Copying an independent scalar or comparing fields observes the
/// item. Only the item may also reach an exactly typed owned destination. Its list root must stay read-only.
fn native_loop_reads(
    statements: &[bir::Statement],
    context: &ReadContext<'_>,
    owned_return: Option<&IncanType>,
) -> bool {
    if !derived_temporaries_are_read_only(statements, context) {
        return false;
    }
    let local = context.local;
    let independent = |value: &bir::Operand| {
        scalar_projection(value, local)
            && matches!(
                operand_type(value, context.locals, context.field_types),
                Some(IncanType::Primitive(_))
            )
    };
    let mut accepted = true;
    walk_neighbors(statements, &mut |statement, next| {
        use bir::StatementKind as S;
        if owned_return.is_some_and(|ty| materializes_item(statement, context, ty)) {
            return;
        }
        accepted &= closure_reads::shared_statement(statement, next, context);
        accepted &= match &statement.kind {
            S::Assign {
                rvalue: bir::Rvalue::Use(value),
                ..
            } => !uses(value, local) || independent(value),
            S::Assign {
                rvalue: bir::Rvalue::Closure { captured_operands, .. },
                ..
            } => !captured_operands.iter().any(|value| uses(value, local)),
            S::Assign {
                rvalue: bir::Rvalue::Match { scrutinee, .. },
                ..
            }
            | S::Assert {
                kind: bir::AssertionKind::Pattern { scrutinee, .. },
                ..
            } => !uses(scrutinee, local) || independent(scrutinee),
            S::Return { value } | S::Break { value } => value
                .as_ref()
                .is_none_or(|value| !uses(value, local) || independent(value)),
            S::Assign {
                rvalue: bir::Rvalue::ResultVariant(variant),
                ..
            } => !uses(&variant.payload, local) || independent(&variant.payload),
            S::Assign {
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, elements),
                ..
            } => elements
                .iter()
                .all(|element| !argument_uses(element, local) || element.as_one().is_some_and(&independent)),
            S::Call { callee, args, .. } => args.iter().all(|argument| {
                !argument_uses(argument, local)
                    || argument
                        .as_one()
                        .is_some_and(|value| independent(value) || read_only_callee(callee))
            }),
            _ => true,
        };
    });
    accepted
}

/// A typed destination that owns the whole checked nominal clones it from the shared item at the use site.
/// Borrowed results, field projections and writes keep the normal observation proof.
fn materializes_item(statement: &bir::Statement, context: &ReadContext<'_>, return_type: &IncanType) -> bool {
    let (operand, expected) = match &statement.kind {
        bir::StatementKind::Return { value: Some(value) } => (value, return_type),
        bir::StatementKind::Assign { place, rvalue } => {
            let Some(destination) = place.local_id().and_then(|id| context.locals.get(id.index())) else {
                return false;
            };
            if !place.projection.is_empty() || destination.origin != bir::LocalOrigin::Temporary {
                return false;
            }
            match rvalue {
                bir::Rvalue::Aggregate(bir::AggregateKind::OptionSome, elements) => {
                    let (Some([ty]), [payload]) = (
                        destination.ty.collection_arguments(CollectionTypeId::Option),
                        elements.as_slice(),
                    ) else {
                        return false;
                    };
                    let Some(value) = payload.as_one() else { return false };
                    (value, ty)
                }
                bir::Rvalue::ResultVariant(variant) => {
                    let Some([ok, err]) = destination.ty.collection_arguments(CollectionTypeId::Result) else {
                        return false;
                    };
                    let expected = match variant.kind {
                        bir::ResultVariantKind::Ok => ok,
                        bir::ResultVariantKind::Err => err,
                    };
                    (&variant.payload, expected)
                }
                _ => return false,
            }
        }
        _ => return false,
    };
    matches!(operand, bir::Operand::Place(read)
        if read.place.local_id() == Some(context.local)
            && read.place.projection.is_empty()
            && matches!(read.fact, bir::OwnershipFact::Move | bir::OwnershipFact::Clone)
            && context.locals[context.local.index()].ty == *expected)
}

/// The temporaries initialized by lending a list, each with the list's root local and element type. A builtin loop's
/// iterator is one of them when the loop neither takes nor changes its items.
fn lent_list_iterators(body: &bir::Body, field_types: &FieldTypes) -> HashMap<bir::LocalId, (bir::LocalId, IncanType)> {
    let locals = &body.locals;
    let mut iterators = HashMap::new();
    walk(&body.block.stmts, &mut |statement| {
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
                // A whole explicit &list keeps its reference here. A field is already resolved on its owner
                // by place_type, using its canonical field identity and checked instantiated layout.
                let owner = match &ty {
                    IncanType::Ref(inner) => inner.as_ref(),
                    other => other,
                };
                let [element] = owner.collection_arguments(CollectionTypeId::List)? else {
                    return None;
                };
                (!matches!(ty, IncanType::Ref(_)) || registered_item(element, body, field_types))
                    .then(|| element.clone())
            })
        {
            iterators.insert(iterator, (root, element));
        }
    });
    iterators
}
