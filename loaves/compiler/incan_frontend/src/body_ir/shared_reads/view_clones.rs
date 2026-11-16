//! RFC 164: an explicit body never fills an ordinary parameter with a silent copy of a borrowed view. Once the
//! shared-read fixed point has decided every callee's realization, a call argument that still clones one of the body's
//! explicit `Borrowed[T]` or `BorrowedMut[T]` inputs, or a projection of one, would satisfy a consuming callee by
//! copying borrowed storage, so the call is refused. A callee that only reads the input receives it shared.

use super::{bir, walk_calls_mut};

/// Refuse each call in an explicit body whose argument clones one of the body's explicit view inputs.
pub(super) fn refuse_cloned_views(body: &mut bir::Body) {
    if body.checked_input_profile.is_none() {
        return;
    }
    let views: Vec<bir::LocalId> = body
        .params
        .iter()
        .filter(|parameter| {
            matches!(
                parameter.input_access,
                incan_semantics_core::IncanInputAccess::Shared | incan_semantics_core::IncanInputAccess::Exclusive
            )
        })
        .map(|parameter| parameter.local)
        .collect();
    if views.is_empty() {
        return;
    }
    walk_calls_mut(&mut body.block.stmts, &mut |statement| {
        let bir::StatementKind::Call { args, .. } = &statement.kind else {
            return;
        };
        let clones_a_view = args.iter().any(|argument| {
            matches!(argument, bir::ArgumentElement::One(bir::Operand::Place(read))
                if read.fact == bir::OwnershipFact::Clone
                    && read.place.local_id().is_some_and(|local| views.contains(&local)))
        });
        if clones_a_view {
            statement.kind = bir::StatementKind::Unsupported {
                description: "a borrowed view passed to a consuming parameter needs an explicit clone (RFC 164)".into(),
            };
        }
    });
}
