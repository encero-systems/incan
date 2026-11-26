//! Match-specific provenance checks for the whole-body shared-read proof, and payload bindings lent to their arm.

use super::*;
use crate::body_ir::SemanticSourceTargetKind;
use incan_semantics_core::SymbolOrigin;

/// A match borrows its scrutinee only when every binding also copies or borrows its selected payload. Move, clone,
/// exclusive and unknown bindings can escape or change storage and therefore retain the owned frame.
pub(super) fn pattern_is_read_only(pattern: &bir::Pattern) -> bool {
    match pattern {
        bir::Pattern::Var(binding) => matches!(binding.fact, bir::OwnershipFact::Copy | bir::OwnershipFact::Borrow),
        bir::Pattern::Tuple(fields) | bir::Pattern::Or(fields) => fields.iter().all(pattern_is_read_only),
        bir::Pattern::Struct { fields, .. } | bir::Pattern::Nominal { fields, .. } => {
            fields.iter().all(|(_, field)| pattern_is_read_only(field))
        }
        bir::Pattern::Result { fields, .. }
        | bir::Pattern::Enum { fields, .. }
        | bir::Pattern::UnionMember { fields, .. } => fields.iter().all(pattern_is_read_only),
        bir::Pattern::Wildcard | bir::Pattern::Literal(_) | bir::Pattern::FieldlessEnumVariant(_) => true,
    }
}

/// Require a carrier pattern whose lowering already dereferences shared enum storage. Structural string, tuple and
/// model matches do not yet accept a shared scrutinee, even when their patterns bind nothing.
pub(super) fn pattern_selects_shared_carrier(pattern: &bir::Pattern) -> bool {
    match pattern {
        bir::Pattern::Result { .. }
        | bir::Pattern::Enum { .. }
        | bir::Pattern::UnionMember { .. }
        | bir::Pattern::FieldlessEnumVariant(_) => true,
        bir::Pattern::Or(alternatives) => alternatives.iter().all(pattern_selects_shared_carrier),
        _ => false,
    }
}

/// Follow every borrowed payload binding through its arm. A binding may be observed, but returning, storing,
/// mutating or capturing it would transfer storage reached through the shared scrutinee.
pub(super) fn match_bindings_are_read_only(arms: &[bir::MatchArm], context: &ReadContext<'_>) -> bool {
    arms.iter().all(|arm| {
        let mut bindings = Vec::new();
        borrowed_pattern_bindings(&arm.pattern, &mut bindings);
        bindings.into_iter().all(|local| {
            arm_only_reads(
                arm,
                &ReadContext {
                    local,
                    cursor: false,
                    ..*context
                },
            )
        })
    })
}

/// Whether one arm's guard, body and result only read `context.local`.
fn arm_only_reads(arm: &bir::MatchArm, context: &ReadContext<'_>) -> bool {
    let local = context.local;
    readonly(&arm.guard_stmts, context)
        && readonly(&arm.body_stmts, context)
        && arm
            .guard
            .as_ref()
            .is_none_or(|guard| !uses(guard, local) || borrowed(guard, local) || scalar_projection(guard, local))
        && (!uses(&arm.result, local) || scalar_projection(&arm.result, local))
}

/// Collect the locals whose pattern facts borrow payloads from the scrutinee. Copy bindings own independent scalar
/// values and need no follow-up ownership proof.
pub(super) fn borrowed_pattern_bindings(pattern: &bir::Pattern, bindings: &mut Vec<bir::LocalId>) {
    match pattern {
        bir::Pattern::Var(binding) if binding.fact == bir::OwnershipFact::Borrow => bindings.push(binding.local),
        bir::Pattern::Tuple(fields) | bir::Pattern::Or(fields) => {
            for field in fields {
                borrowed_pattern_bindings(field, bindings);
            }
        }
        bir::Pattern::Struct { fields, .. } | bir::Pattern::Nominal { fields, .. } => {
            for (_, field) in fields {
                borrowed_pattern_bindings(field, bindings);
            }
        }
        bir::Pattern::Result { fields, .. }
        | bir::Pattern::Enum { fields, .. }
        | bir::Pattern::UnionMember { fields, .. } => {
            for field in fields {
                borrowed_pattern_bindings(field, bindings);
            }
        }
        _ => {}
    }
}

/// Bind a payload by reference when its arm only reads it (#1607), instead of copying it into an owned binding that
/// the arm then drops. A binding qualifies when it takes a copy of a whole payload of an enum, `Option` or `Result`
/// pattern at the arm's top, its type is text, a list or a model or enum, and its arm only reads it. The scrutinee must
/// be a local or fields of one that the arm also only reads, so the payload stays in place until the arm ends. The
/// binding's local becomes a reference to the payload's type and its fact a borrow; a call into a shared parameter then
/// lends that reference. This runs on final signatures, so no callable's input changes.
pub(super) fn retain_borrowed_payload_bindings(
    body: &mut bir::Body,
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    // An open payload type has no settled shared form, and a loan must not be held across a suspension.
    if !body.type_parameters.is_empty()
        || !body.nominal_instantiation.is_empty()
        || body.trait_default_instantiation.is_some()
        || body.is_async
        || body.is_generator()
    {
        return;
    }
    let mut lent = Vec::new();
    walk(&body.block.stmts, &mut |statement| {
        let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Match { scrutinee, arms },
            ..
        } = &statement.kind
        else {
            return;
        };
        let Some(root) = field_scrutinee_root(scrutinee) else {
            return;
        };
        let context = |local| ReadContext {
            locals: &body.locals,
            signatures,
            field_types,
            candidates,
            local,
            cursor: false,
            derived: false,
        };
        for arm in arms {
            let bindings = copied_payload_bindings(&arm.pattern, &body.locals, &body.named_type_identities);
            if !bindings.is_empty() && arm_only_reads(arm, &context(root)) {
                lent.extend(bindings.into_iter().filter(|&local| {
                    arm_only_reads(arm, &context(local)) && !arm_formats_reference(arm, local, &body.locals)
                }));
            }
        }
    });
    if lent.is_empty() {
        return;
    }
    lend_locals(body, &lent, signatures, field_types, candidates);
    walk_mut(&mut body.block.stmts, &mut |statement| {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Match { arms, .. },
            ..
        } = &mut statement.kind
        {
            for arm in arms {
                lend_payload_bindings(&mut arm.pattern, &lent);
            }
        }
    });
}

/// The local a scrutinee reads, when it is a local or fields of one; an index, a slice or a global keeps the copy.
fn field_scrutinee_root(scrutinee: &bir::Operand) -> Option<bir::LocalId> {
    let bir::Operand::Place(read) = scrutinee else {
        return None;
    };
    read.place
        .projection
        .iter()
        .all(|step| matches!(step, bir::PlaceElem::Field { .. }))
        .then(|| read.place.local_id())
        .flatten()
}

/// The bindings of a top-level enum, `Option` or `Result` pattern that copy a whole lendable payload.
fn copied_payload_bindings(
    pattern: &bir::Pattern,
    locals: &[bir::LocalDecl],
    identities: &bir::SharedIdentities,
) -> Vec<bir::LocalId> {
    let (bir::Pattern::Enum { fields, .. } | bir::Pattern::Result { fields, .. }) = pattern else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|field| match field {
            bir::Pattern::Var(binding)
                if matches!(binding.fact, bir::OwnershipFact::Clone | bir::OwnershipFact::Borrow)
                    && locals
                        .get(binding.local.index())
                        .is_some_and(|local| lendable_payload(&local.ty, identities)) =>
            {
                Some(binding.local)
            }
            _ => None,
        })
        .collect()
}

/// Text, a list, or a model or enum: a payload lowering lends in place. A native item keeps its checked payload
/// transfer.
fn lendable_payload(ty: &IncanType, identities: &bir::SharedIdentities) -> bool {
    lendable_value(ty, identities) || ty.collection_arguments(CollectionTypeId::List).is_some()
}

/// Text, or a model or enum of this project or a library, selected by its retained identity rather than its spelling:
/// a value lowering lends as a shared reference to it.
pub(super) fn lendable_value(ty: &IncanType, identities: &bir::SharedIdentities) -> bool {
    match ty {
        IncanType::Primitive(IncanPrimitiveType::Str) => true,
        IncanType::Named(name) => identities.get(name).is_some_and(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Enum
            ) && matches!(identity.origin, SymbolOrigin::Module(_) | SymbolOrigin::Package { .. })
        }),
        _ => false,
    }
}

/// Rewrite the lent bindings of one arm's top-level payload pattern as borrows of their payload.
fn lend_payload_bindings(pattern: &mut bir::Pattern, lent: &[bir::LocalId]) {
    let (bir::Pattern::Enum { fields, .. } | bir::Pattern::Result { fields, .. }) = pattern else {
        return;
    };
    for field in fields {
        if let bir::Pattern::Var(binding) = field
            && lent.contains(&binding.local)
        {
            binding.fact = bir::OwnershipFact::Borrow;
            binding.last_use = false;
        }
    }
}

/// Give each lent local a reference to its type. A text local's compiler temporaries become loans of it first, as a
/// lent text parameter's do, while its type still names the owner they would otherwise copy.
pub(super) fn lend_locals(
    body: &mut bir::Body,
    lent: &[bir::LocalId],
    signatures: &Signatures,
    field_types: &FieldTypes,
    candidates: &HashSet<Candidate>,
) {
    for &local in lent {
        retain_shared_temporaries(body, local, signatures, field_types, candidates);
    }
    for local in body.locals.iter_mut().filter(|local| lent.contains(&local.id)) {
        local.ty = IncanType::Ref(Box::new(local.ty.clone()));
    }
}

/// Whether an arm's guard or body formats `local` other than as whole text displayed.
fn arm_formats_reference(arm: &bir::MatchArm, local: bir::LocalId, locals: &[bir::LocalDecl]) -> bool {
    formats_reference(&arm.guard_stmts, local, locals) || formats_reference(&arm.body_stmts, local, locals)
}

/// Whether an interpolation reads `local` other than as whole text displayed. Lowering formats a reference local by
/// another route than an owned one, and that route admits no other style or projection yet, so such a local keeps
/// its owned copy.
pub(super) fn formats_reference(statements: &[bir::Statement], local: bir::LocalId, locals: &[bir::LocalDecl]) -> bool {
    let text = locals
        .get(local.index())
        .is_some_and(|declaration| declaration.ty == IncanType::Primitive(IncanPrimitiveType::Str));
    let mut found = false;
    walk(statements, &mut |statement| {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Format(parts),
            ..
        } = &statement.kind
        {
            found |= parts.iter().any(|part| {
                matches!(part, bir::FormatPart::Expr { operand, style } if uses(operand, local)
                    && !(text && *style == bir::FormatStyle::Display
                        && matches!(operand.as_ref(), bir::Operand::Place(read) if read.place.projection.is_empty())))
            });
        }
    });
    found
}
