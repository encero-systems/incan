//! Closed result shapes that source-call projection can classify without retained dependencies.

use crate::symbols::ResolvedType;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

/// This initial call profile admits only closed scalar/owned byte-string results and builtin `Result`/`Option`
/// carriers of them, with no possible retained source dependency. Other aggregates, references, native and generic
/// results await their checked dependency representation.
pub(super) fn plain_result(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Unit
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Numeric(_)
        | ResolvedType::Bool
        | ResolvedType::Str
        | ResolvedType::Bytes
        | ResolvedType::FrozenStr
        | ResolvedType::FrozenBytes => true,
        ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
            !arguments.is_empty() && arguments.iter().all(plain_result)
        }
        _ => false,
    }
}

/// Retain an unresolved scalar-carrier obligation without declaring any named leaf independent. Legacy checking
/// can represent an enclosing binder as Named; Body IR classifies it using the exact declaration's binder inventory.
/// A real nominal leaf stays nominal and refuses closed admission until its dependency proof is supported.
pub(in crate::typechecker::ownership_contracts) fn conditional_result(ty: &ResolvedType) -> bool {
    /// Check carrier leaves while retaining whether any unresolved binder prevents a final independence proof.
    fn leaves(ty: &ResolvedType, generic: &mut bool) -> bool {
        match ty {
            ResolvedType::TypeVar(_) | ResolvedType::Named(_) => {
                *generic = true;
                true
            }
            ResolvedType::Generic(_, arguments) if ty.is_result() || ty.is_option() => {
                !arguments.is_empty() && arguments.iter().all(|item| leaves(item, generic))
            }
            _ => plain_result(ty),
        }
    }
    let mut generic = false;
    (ty.is_result() || ty.is_option()) && leaves(ty, &mut generic) && generic
}

/// Printing reaches no nominal display hook for primitive values and for lists whose items print the same way.
pub(super) fn primitive_display(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Str
        | ResolvedType::FrozenStr
        | ResolvedType::Int
        | ResolvedType::Float
        | ResolvedType::Bool
        | ResolvedType::Unit => true,
        ResolvedType::Generic(name, arguments) => {
            collections::from_str(name) == Some(CollectionTypeId::List)
                && matches!(arguments.as_slice(), [item] if primitive_display(item))
        }
        _ => false,
    }
}
