//! Type name constants and generic constructors used across the typechecker.
use crate::symbols::ResolvedType;
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::numerics::{self, DecimalTypeConstructorId};
use incan_lang::lang::types::stringlike::{self, StringLikeId};

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

/// The constructor, precision and scale of a checked decimal type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimalShape {
    /// `decimal` (with its alias `numeric`) or `decimal128`; the two are separate types.
    pub constructor: DecimalTypeConstructorId,
    /// The total digit count `p`.
    pub precision: u8,
    /// The fractional digit count `s`.
    pub scale: u8,
}

/// Return the shape of a checked `decimal[p, s]`, `numeric[p, s]` or `decimal128[p, s]` type.
///
/// A checked annotation resolves to the canonical constructor with its precision and scale carried as integer
/// spellings in type-argument position. Anything else, including a decimal whose arguments are not integer spellings,
/// has no shape.
pub fn decimal_shape(ty: &ResolvedType) -> Option<DecimalShape> {
    let ResolvedType::Generic(name, args) = ty else {
        return None;
    };
    let constructor = numerics::decimal_constructor_from_str(name.as_str())?;
    let [precision, scale] = args.as_slice() else {
        return None;
    };
    let digit_count = |arg: &ResolvedType| match arg {
        ResolvedType::TypeVar(value) => value.parse::<u8>().ok(),
        _ => None,
    };
    Some(DecimalShape {
        constructor,
        precision: digit_count(precision)?,
        scale: digit_count(scale)?,
    })
}

/// Decide whether a value of one checked decimal type is assignable to another, or `None` when either is not one.
///
/// Precision and scale ride in type-argument position as type variables, which the generic compatibility rules would
/// match against anything. A decimal value is assignable only to a decimal type of the same constructor that keeps at
/// least its digits before the point (`p - s`) and its scale `s`, so the assignment is provably lossless (#1809).
pub fn decimal_types_compatible(actual: &ResolvedType, expected: &ResolvedType) -> Option<bool> {
    let (actual, expected) = (decimal_shape(actual)?, decimal_shape(expected)?);
    Some(
        actual.constructor == expected.constructor
            && incan_lang::numeric_values::decimal_type_losslessly_widens_to(
                actual.precision,
                actual.scale,
                expected.precision,
                expected.scale,
            ),
    )
}
