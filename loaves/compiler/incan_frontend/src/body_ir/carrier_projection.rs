//! Preserve checker-proven narrowed reads of nested Option and union storage (#1337, #1698).

use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{
    IncanType,
    body_ir::{Place, PlaceElem},
};

/// Retain ordered payload steps only when the checker's read type is contained in the exact physical storage type.
/// Checking owns the narrowing proof; this bridge neither inspects enclosing branches nor changes ownership facts.
pub(super) fn retain_checked_narrowing(place: &mut Place, storage: &IncanType, checked: &IncanType) {
    if storage == checked {
        return;
    }
    if let Some([payload]) = storage.collection_arguments(CollectionTypeId::Option) {
        if contains_checked_type(payload, checked) {
            place.projection.push(PlaceElem::OptionPayload {
                option_type: storage.clone(),
                payload_type: payload.clone(),
            });
            retain_checked_narrowing(place, payload, checked);
        }
        return;
    }
    if let Some(args) = storage.union_members() {
        if args.contains(checked) {
            place.projection.push(PlaceElem::UnionMember { ty: checked.clone() });
            return;
        }
        let mut candidates = args.iter().filter(|member| contains_checked_type(member, checked));
        if let Some(member) = candidates.next()
            && candidates.next().is_none()
        {
            place.projection.push(PlaceElem::UnionMember { ty: member.clone() });
            retain_checked_narrowing(place, member, checked);
        }
    }
}

/// Recognize exact nested carrier membership, without admitting names, inferred types or a missing checker result.
fn contains_checked_type(storage: &IncanType, checked: &IncanType) -> bool {
    if storage == checked {
        return true;
    }
    if let Some([payload]) = storage.collection_arguments(CollectionTypeId::Option) {
        return contains_checked_type(payload, checked);
    }
    storage
        .union_members()
        .is_some_and(|members| members.iter().any(|member| contains_checked_type(member, checked)))
}
