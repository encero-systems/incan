//! Validate checked payload instantiation without rerunning inference, coercion or name resolution.

use std::collections::BTreeMap;

use crate::symbols::ResolvedType;

/// Compare retained declaration structure with an already checked instance. Binders close consistently; all
/// fixed components retain exact checked identity. This relation never infers a caller's missing argument type.
pub(super) fn matches(template: &ResolvedType, actual: &ResolvedType) -> bool {
    matches_with(template, actual, &mut BTreeMap::new())
}

/// Retain one binding per declaration variable across the complete payload, including nested callable types.
fn matches_with<'a>(
    template: &ResolvedType,
    actual: &'a ResolvedType,
    bindings: &mut BTreeMap<String, &'a ResolvedType>,
) -> bool {
    if matches!(actual, ResolvedType::Unknown | ResolvedType::CallSiteInfer) {
        return false;
    }
    match (template, actual) {
        (ResolvedType::TypeVar(name), _) => match bindings.get(name) {
            Some(previous) => *previous == actual,
            None => {
                bindings.insert(name.clone(), actual);
                true
            }
        },
        (ResolvedType::Generic(name, left), ResolvedType::Generic(other, right)) => {
            name == other && components_match(left, right, bindings)
        }
        (ResolvedType::Tuple(left), ResolvedType::Tuple(right)) => components_match(left, right, bindings),
        (ResolvedType::Function(left, result), ResolvedType::Function(right, other_result)) => {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| {
                    left.name == right.name
                        && left.kind == right.kind
                        && left.has_default == right.has_default
                        && left.is_partial_preset == right.is_partial_preset
                        && left.is_mut == right.is_mut
                        && matches_with(&left.ty, &right.ty, bindings)
                })
                && matches_with(result, other_result, bindings)
        }
        (ResolvedType::TypeToken(left), ResolvedType::TypeToken(right))
        | (ResolvedType::FrozenList(left), ResolvedType::FrozenList(right))
        | (ResolvedType::FrozenSet(left), ResolvedType::FrozenSet(right))
        | (ResolvedType::Ref(left), ResolvedType::Ref(right))
        | (ResolvedType::RefMut(left), ResolvedType::RefMut(right)) => matches_with(left, right, bindings),
        (ResolvedType::FrozenDict(lkey, lvalue), ResolvedType::FrozenDict(rkey, rvalue)) => {
            matches_with(lkey, rkey, bindings) && matches_with(lvalue, rvalue, bindings)
        }
        _ => template == actual,
    }
}

/// Structural arity is checked before pairing components; a short zip cannot hide missing payload structure.
fn components_match<'a>(
    template: &[ResolvedType],
    actual: &'a [ResolvedType],
    bindings: &mut BTreeMap<String, &'a ResolvedType>,
) -> bool {
    template.len() == actual.len()
        && template
            .iter()
            .zip(actual)
            .all(|(left, right)| matches_with(left, right, bindings))
}
