//! Match-specific provenance checks for the whole-body shared-read proof.

use super::*;

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
            let binding = ReadContext {
                locals: context.locals,
                signatures: context.signatures,
                field_types: context.field_types,
                candidates: context.candidates,
                local,
                cursor: false,
                derived: context.derived,
            };
            readonly(&arm.guard_stmts, &binding)
                && readonly(&arm.body_stmts, &binding)
                && arm.guard.as_ref().is_none_or(|guard| {
                    !uses(guard, local) || borrowed(guard, local) || scalar_projection(guard, local)
                })
                && (!uses(&arm.result, local) || scalar_projection(&arm.result, local))
        })
    })
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
