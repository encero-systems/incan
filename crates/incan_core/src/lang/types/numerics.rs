//! Numeric builtin type vocabulary.
//!
//! This registry covers builtin numeric (and numeric-adjacent) type names and their aliases.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-insensitive ASCII**.
//! - This module is vocabulary only (spellings + metadata), not type-system semantics.
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::types::numerics::{self, NumericTypeId};
//!
//! assert_eq!(numerics::from_str("int"), Some(NumericTypeId::I64));
//! assert_eq!(numerics::from_str("I64"), Some(NumericTypeId::I64));
//! assert_eq!(numerics::as_str(NumericTypeId::F64), "f64");
//! ```

use crate::lang::registry::{Example, RFC, RfcId, Since, Stability};

/// Stable identifier for numeric builtin types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericTypeId {
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    ISize,
    USize,
    Bool,
}

/// Numeric family represented by a builtin numeric type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericFamily {
    SignedInteger,
    UnsignedInteger,
    BinaryFloat,
    Bool,
}

/// Metadata for a numeric builtin type.
#[derive(Debug, Clone, Copy)]
pub struct NumericTypeInfo {
    pub id: NumericTypeId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub family: NumericFamily,
    pub bit_width: Option<u16>,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Decimal type constructor accepted in parameterized type position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecimalTypeConstructorId {
    Decimal,
    Decimal128,
}

/// Metadata for a decimal type constructor.
#[derive(Debug, Clone, Copy)]
pub struct DecimalTypeConstructorInfo {
    pub id: DecimalTypeConstructorId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub max_precision: u8,
    pub storage_bits: u16,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
}

/// Registry of numeric builtin types.
pub const NUMERIC_TYPES: &[NumericTypeInfo] = &[
    info(
        NumericTypeId::I8,
        "i8",
        &[],
        "Signed 8-bit integer type.",
        NumericFamily::SignedInteger,
        Some(8),
    ),
    info(
        NumericTypeId::I16,
        "i16",
        &["short", "smallint"],
        "Signed 16-bit integer type.",
        NumericFamily::SignedInteger,
        Some(16),
    ),
    info(
        NumericTypeId::I32,
        "i32",
        &["integer"],
        "Signed 32-bit integer type.",
        NumericFamily::SignedInteger,
        Some(32),
    ),
    info(
        NumericTypeId::I64,
        "i64",
        &["int", "bigint", "long"],
        "Signed 64-bit integer type.",
        NumericFamily::SignedInteger,
        Some(64),
    ),
    info(
        NumericTypeId::I128,
        "i128",
        &["hugeint"],
        "Signed 128-bit integer type.",
        NumericFamily::SignedInteger,
        Some(128),
    ),
    info(
        NumericTypeId::U8,
        "u8",
        &["byte"],
        "Unsigned 8-bit integer type.",
        NumericFamily::UnsignedInteger,
        Some(8),
    ),
    info(
        NumericTypeId::U16,
        "u16",
        &[],
        "Unsigned 16-bit integer type.",
        NumericFamily::UnsignedInteger,
        Some(16),
    ),
    info(
        NumericTypeId::U32,
        "u32",
        &[],
        "Unsigned 32-bit integer type.",
        NumericFamily::UnsignedInteger,
        Some(32),
    ),
    info(
        NumericTypeId::U64,
        "u64",
        &[],
        "Unsigned 64-bit integer type.",
        NumericFamily::UnsignedInteger,
        Some(64),
    ),
    info(
        NumericTypeId::U128,
        "u128",
        &[],
        "Unsigned 128-bit integer type.",
        NumericFamily::UnsignedInteger,
        Some(128),
    ),
    info(
        NumericTypeId::F32,
        "f32",
        &["real", "fp32"],
        "32-bit binary floating-point type.",
        NumericFamily::BinaryFloat,
        Some(32),
    ),
    info(
        NumericTypeId::F64,
        "f64",
        &["float", "double", "fp64"],
        "64-bit binary floating-point type.",
        NumericFamily::BinaryFloat,
        Some(64),
    ),
    info(
        NumericTypeId::ISize,
        "isize",
        &[],
        "Pointer-sized signed integer type.",
        NumericFamily::SignedInteger,
        None,
    ),
    info(
        NumericTypeId::USize,
        "usize",
        &[],
        "Pointer-sized unsigned integer type.",
        NumericFamily::UnsignedInteger,
        None,
    ),
    legacy_info(
        NumericTypeId::Bool,
        "bool",
        &[],
        "Builtin boolean type.",
        NumericFamily::Bool,
        None,
        RFC::_000,
        Since(0, 1),
    ),
];

/// Registry of decimal type constructors.
pub const DECIMAL_TYPE_CONSTRUCTORS: &[DecimalTypeConstructorInfo] = &[
    decimal_constructor(DecimalTypeConstructorId::Decimal, "decimal", &["numeric"], 38, 128),
    decimal_constructor(DecimalTypeConstructorId::Decimal128, "decimal128", &[], 38, 128),
];

/// Resolve a type name to a [`NumericTypeId`].
///
/// ## Parameters
/// - `name`: Candidate type name (canonical or alias).
///
/// ## Returns
/// - `Some(NumericTypeId)` if the spelling matches this registry.
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-insensitive ASCII**.
pub fn from_str(name: &str) -> Option<NumericTypeId> {
    if let Some(t) = NUMERIC_TYPES.iter().find(|t| t.canonical.eq_ignore_ascii_case(name)) {
        return Some(t.id);
    }
    NUMERIC_TYPES
        .iter()
        .find(|t| t.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .map(|t| t.id)
}

/// Resolve a parameterized decimal type constructor spelling.
///
/// Bare decimal names are reserved outside parameterized type position; callers should use this helper only when they
/// are resolving a generic type constructor such as `decimal[P, S]`.
pub fn decimal_constructor_from_str(name: &str) -> Option<DecimalTypeConstructorId> {
    if let Some(t) = DECIMAL_TYPE_CONSTRUCTORS
        .iter()
        .find(|t| t.canonical.eq_ignore_ascii_case(name))
    {
        return Some(t.id);
    }
    DECIMAL_TYPE_CONSTRUCTORS
        .iter()
        .find(|t| t.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .map(|t| t.id)
}

/// Return the canonical spelling for a numeric builtin type.
///
/// ## Parameters
/// - `id`: Numeric type identifier.
///
/// ## Returns
/// - The canonical spelling (e.g. `"int"`).
pub fn as_str(id: NumericTypeId) -> &'static str {
    info_for(id).canonical
}

/// Return the Rust primitive spelling for a numeric builtin type.
pub fn rust_name(id: NumericTypeId) -> &'static str {
    as_str(id)
}

/// Return whether `id` is an integer family numeric.
pub fn is_integer(id: NumericTypeId) -> bool {
    matches!(
        info_for(id).family,
        NumericFamily::SignedInteger | NumericFamily::UnsignedInteger
    )
}

/// Return whether `id` is a binary floating-point numeric.
pub fn is_binary_float(id: NumericTypeId) -> bool {
    matches!(info_for(id).family, NumericFamily::BinaryFloat)
}

/// Return the full metadata entry for a numeric builtin type.
///
/// ## Parameters
/// - `id`: Numeric type identifier.
///
/// ## Returns
/// - The associated [`NumericTypeInfo`] copied from [`NUMERIC_TYPES`].
///
/// The lookup is exhaustive over the closed enum, so adding a numeric type requires updating this match at compile
/// time.
pub fn info_for(id: NumericTypeId) -> NumericTypeInfo {
    match id {
        NumericTypeId::I8 => NUMERIC_TYPES[0],
        NumericTypeId::I16 => NUMERIC_TYPES[1],
        NumericTypeId::I32 => NUMERIC_TYPES[2],
        NumericTypeId::I64 => NUMERIC_TYPES[3],
        NumericTypeId::I128 => NUMERIC_TYPES[4],
        NumericTypeId::U8 => NUMERIC_TYPES[5],
        NumericTypeId::U16 => NUMERIC_TYPES[6],
        NumericTypeId::U32 => NUMERIC_TYPES[7],
        NumericTypeId::U64 => NUMERIC_TYPES[8],
        NumericTypeId::U128 => NUMERIC_TYPES[9],
        NumericTypeId::F32 => NUMERIC_TYPES[10],
        NumericTypeId::F64 => NUMERIC_TYPES[11],
        NumericTypeId::ISize => NUMERIC_TYPES[12],
        NumericTypeId::USize => NUMERIC_TYPES[13],
        NumericTypeId::Bool => NUMERIC_TYPES[14],
    }
}

/// Return the full metadata entry for a decimal type constructor.
pub fn decimal_constructor_info_for(id: DecimalTypeConstructorId) -> DecimalTypeConstructorInfo {
    match id {
        DecimalTypeConstructorId::Decimal => DECIMAL_TYPE_CONSTRUCTORS[0],
        DecimalTypeConstructorId::Decimal128 => DECIMAL_TYPE_CONSTRUCTORS[1],
    }
}

/// Build RFC 009 metadata for a canonical numeric type.
const fn info(
    id: NumericTypeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    family: NumericFamily,
    bit_width: Option<u16>,
) -> NumericTypeInfo {
    NumericTypeInfo {
        id,
        canonical,
        aliases,
        description,
        family,
        bit_width,
        introduced_in_rfc: RFC::_009,
        since: Since(0, 3),
        stability: Stability::Stable,
        examples: &[],
    }
}

/// Build metadata for numeric spellings that existed before the RFC 009 registry expansion.
#[allow(clippy::too_many_arguments)]
const fn legacy_info(
    id: NumericTypeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    family: NumericFamily,
    bit_width: Option<u16>,
    introduced_in_rfc: RfcId,
    since: Since,
) -> NumericTypeInfo {
    NumericTypeInfo {
        id,
        canonical,
        aliases,
        description,
        family,
        bit_width,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}

/// Build metadata for a parameterized decimal type constructor.
const fn decimal_constructor(
    id: DecimalTypeConstructorId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    max_precision: u8,
    storage_bits: u16,
) -> DecimalTypeConstructorInfo {
    DecimalTypeConstructorInfo {
        id,
        canonical,
        aliases,
        max_precision,
        storage_bits,
        introduced_in_rfc: RFC::_009,
        since: Since(0, 3),
        stability: Stability::Stable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_canonical_exact_width_numeric_names() {
        assert_eq!(from_str("i8"), Some(NumericTypeId::I8));
        assert_eq!(from_str("i16"), Some(NumericTypeId::I16));
        assert_eq!(from_str("i32"), Some(NumericTypeId::I32));
        assert_eq!(from_str("i64"), Some(NumericTypeId::I64));
        assert_eq!(from_str("i128"), Some(NumericTypeId::I128));
        assert_eq!(from_str("u8"), Some(NumericTypeId::U8));
        assert_eq!(from_str("u16"), Some(NumericTypeId::U16));
        assert_eq!(from_str("u32"), Some(NumericTypeId::U32));
        assert_eq!(from_str("u64"), Some(NumericTypeId::U64));
        assert_eq!(from_str("u128"), Some(NumericTypeId::U128));
        assert_eq!(from_str("f32"), Some(NumericTypeId::F32));
        assert_eq!(from_str("f64"), Some(NumericTypeId::F64));
        assert_eq!(from_str("isize"), Some(NumericTypeId::ISize));
        assert_eq!(from_str("usize"), Some(NumericTypeId::USize));
    }

    #[test]
    fn resolves_data_oriented_numeric_aliases() {
        assert_eq!(from_str("int"), Some(NumericTypeId::I64));
        assert_eq!(from_str("bigint"), Some(NumericTypeId::I64));
        assert_eq!(from_str("long"), Some(NumericTypeId::I64));
        assert_eq!(from_str("hugeint"), Some(NumericTypeId::I128));
        assert_eq!(from_str("integer"), Some(NumericTypeId::I32));
        assert_eq!(from_str("short"), Some(NumericTypeId::I16));
        assert_eq!(from_str("smallint"), Some(NumericTypeId::I16));
        assert_eq!(from_str("byte"), Some(NumericTypeId::U8));
        assert_eq!(from_str("float"), Some(NumericTypeId::F64));
        assert_eq!(from_str("double"), Some(NumericTypeId::F64));
        assert_eq!(from_str("real"), Some(NumericTypeId::F32));
        assert_eq!(from_str("fp32"), Some(NumericTypeId::F32));
        assert_eq!(from_str("fp64"), Some(NumericTypeId::F64));
    }

    #[test]
    fn resolves_decimal_constructors_separately_from_bare_numeric_types() {
        assert_eq!(from_str("decimal"), None);
        assert_eq!(from_str("numeric"), None);
        assert_eq!(from_str("decimal128"), None);
        assert_eq!(
            decimal_constructor_from_str("decimal"),
            Some(DecimalTypeConstructorId::Decimal)
        );
        assert_eq!(
            decimal_constructor_from_str("numeric"),
            Some(DecimalTypeConstructorId::Decimal)
        );
        assert_eq!(
            decimal_constructor_from_str("decimal128"),
            Some(DecimalTypeConstructorId::Decimal128)
        );
    }

    #[test]
    fn exposes_numeric_family_and_width_metadata() {
        let i32_info = info_for(NumericTypeId::I32);
        assert_eq!(i32_info.family, NumericFamily::SignedInteger);
        assert_eq!(i32_info.bit_width, Some(32));
        assert_eq!(i32_info.introduced_in_rfc, RFC::_009);

        let u8_info = info_for(NumericTypeId::U8);
        assert_eq!(u8_info.family, NumericFamily::UnsignedInteger);
        assert_eq!(u8_info.bit_width, Some(8));

        let f64_info = info_for(NumericTypeId::F64);
        assert_eq!(f64_info.family, NumericFamily::BinaryFloat);
        assert_eq!(f64_info.bit_width, Some(64));

        let bool_info = info_for(NumericTypeId::Bool);
        assert_eq!(bool_info.family, NumericFamily::Bool);
        assert_eq!(bool_info.introduced_in_rfc, RFC::_000);
        assert_eq!(bool_info.since, Since(0, 1));
    }
}
