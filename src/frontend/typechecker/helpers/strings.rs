//! String-related helpers for the typechecker (predicates, method returns).
use crate::frontend::symbols::ResolvedType;

use super::{list_ty, stringlike_type_id};
use incan_core::lang::surface::string_methods::{self, StringMethodId};
use incan_core::lang::types::stringlike::StringLikeId;

/// Check whether a resolved type should be treated as string-like.
///
/// This returns `true` for:
/// - `str` (runtime string)
/// - `FrozenStr` (const-eval / frozen string)
pub fn is_str_like(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::Str | ResolvedType::FrozenStr)
        || matches!(ty, ResolvedType::Named(name) if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr))
}

/// Check whether a resolved type is `FrozenStr`.
pub fn is_frozen_str(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::FrozenStr)
        || matches!(ty, ResolvedType::Named(name) if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenStr))
}

/// Construct the resolved type `FrozenStr`.
pub fn frozen_str_ty() -> ResolvedType {
    ResolvedType::FrozenStr
}

/// Check whether a resolved type is `FrozenBytes`.
pub fn is_frozen_bytes(ty: &ResolvedType) -> bool {
    matches!(ty, ResolvedType::FrozenBytes)
        || matches!(ty, ResolvedType::Named(name) if stringlike_type_id(name.as_str()) == Some(StringLikeId::FrozenBytes))
}

/// Construct the resolved type `FrozenBytes`.
pub fn frozen_bytes_ty() -> ResolvedType {
    ResolvedType::FrozenBytes
}

/// Return one canonical string-method identity and its resolved result type, if the receiver admits it.
///
/// Returning the identity beside the type lets a later compiler stage retain the typechecker's selected operation
/// without resolving the source spelling a second time. Callers that only need the result type should use
/// [`string_method_return`].
pub fn string_method_identity_and_return(method: &str, include_len: bool) -> Option<(StringMethodId, ResolvedType)> {
    let id = string_methods::from_str(method)?;
    let result = string_method_return_for_id(id, include_len)?;
    Some((id, result))
}

/// Return the resolved type for a supported string method, if known.
pub fn string_method_return(method: &str, include_len: bool) -> Option<ResolvedType> {
    string_method_identity_and_return(method, include_len).map(|(_, result)| result)
}

/// Return the resolved type associated with one already-resolved string-method identity.
fn string_method_return_for_id(id: StringMethodId, include_len: bool) -> Option<ResolvedType> {
    match id {
        StringMethodId::Upper
        | StringMethodId::Lower
        | StringMethodId::Strip
        | StringMethodId::Replace
        | StringMethodId::Join
        | StringMethodId::ToString => Some(ResolvedType::Str),
        StringMethodId::SplitWhitespace | StringMethodId::Split => Some(list_ty(ResolvedType::Str)),
        StringMethodId::Contains | StringMethodId::StartsWith | StringMethodId::EndsWith => Some(ResolvedType::Bool),
        StringMethodId::Len if include_len => Some(ResolvedType::Int),
        StringMethodId::IsEmpty if include_len => Some(ResolvedType::Bool),
        _ => None,
    }
}
