//! Type name constants and generic constructors used across the typechecker.
use crate::symbols::ResolvedType;
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::stringlike::{self, StringLikeId};

pub use crate::symbols::render_resolved_type_as_rust_arg;

/// Resolve a collection/generic-base type name (canonical or alias) to its stable id.
pub fn collection_type_id(name: &str) -> Option<CollectionTypeId> {
    collections::from_str(name)
}

/// Resolve a string-like builtin type name (canonical or alias) to its stable id.
pub fn stringlike_type_id(name: &str) -> Option<StringLikeId> {
    stringlike::from_str(name)
}

/// Return the canonical spelling for a collection/generic-base builtin type.
pub fn collection_name(id: CollectionTypeId) -> &'static str {
    collections::as_str(id)
}

/// Construct a `List[T]` type.
///
/// ## Parameters
/// - `elem`: The element type `T`.
///
/// ## Returns
/// - The resolved type `List[T]`.
pub fn list_ty(elem: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::List).to_string(), vec![elem])
}

/// Construct a `Dict[K, V]` type.
///
/// ## Parameters
/// - `key`: The key type `K`.
/// - `val`: The value type `V`.
///
/// ## Returns
/// - The resolved type `Dict[K, V]`.
pub fn dict_ty(key: ResolvedType, val: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Dict).to_string(), vec![key, val])
}

/// Construct a `Set[T]` type.
pub fn set_ty(elem: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Set).to_string(), vec![elem])
}

/// Construct an `Option[T]` type.
///
/// ## Parameters
/// - `inner`: The inner type `T`.
///
/// ## Returns
/// - The resolved type `Option[T]`.
pub fn option_ty(inner: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Option).to_string(), vec![inner])
}

/// Construct a `Result[Ok, Err]` type.
///
/// ## Parameters
/// - `ok`: The ok type.
/// - `err`: The error type.
///
/// ## Returns
/// - The resolved type `Result[Ok, Err]`.
pub fn result_ty(ok: ResolvedType, err: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Result).to_string(), vec![ok, err])
}

/// Construct a `Generator[T]` type.
pub fn generator_ty(elem: ResolvedType) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Generator).to_string(), vec![elem])
}

/// Construct a `Tuple[T1, T2, ...]` generic type (when used in generic form).
pub fn tuple_generic_ty(elems: Vec<ResolvedType>) -> ResolvedType {
    ResolvedType::Generic(collection_name(CollectionTypeId::Tuple).to_string(), elems)
}
