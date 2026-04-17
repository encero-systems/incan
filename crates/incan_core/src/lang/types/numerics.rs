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
//! assert_eq!(numerics::from_str("int"), Some(NumericTypeId::Int));
//! assert_eq!(numerics::from_str("I64"), Some(NumericTypeId::Int));
//! assert_eq!(numerics::as_str(NumericTypeId::Float), "float");
//! ```

use crate::lang::registry::{Example, RFC, RfcId, Since, Stability};

/// Stable identifier for numeric builtin types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericTypeId {
    Int,
    Float,
    Bool,
}

/// Metadata for a numeric builtin type.
#[derive(Debug, Clone, Copy)]
pub struct NumericTypeInfo {
    pub id: NumericTypeId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of numeric builtin types.
pub const NUMERIC_TYPES: &[NumericTypeInfo] = &[
    info(
        NumericTypeId::Int,
        "int",
        &["i64", "i32"],
        "Builtin signed integer type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        NumericTypeId::Float,
        "float",
        &["f64", "f32"],
        "Builtin floating-point type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        NumericTypeId::Bool,
        "bool",
        &[],
        "Builtin boolean type.",
        RFC::_000,
        Since(0, 1),
    ),
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
        NumericTypeId::Int => NUMERIC_TYPES[0],
        NumericTypeId::Float => NUMERIC_TYPES[1],
        NumericTypeId::Bool => NUMERIC_TYPES[2],
    }
}

const fn info(
    id: NumericTypeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> NumericTypeInfo {
    NumericTypeInfo {
        id,
        canonical,
        aliases,
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
