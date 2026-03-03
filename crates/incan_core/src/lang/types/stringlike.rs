//! String-like builtin type vocabulary.
//!
//! This registry covers builtin type names related to strings/bytes (and their aliases).
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-insensitive ASCII**.
//! - This module is vocabulary only (spellings + metadata), not type-system semantics.
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::types::stringlike::{self, StringLikeId};
//!
//! assert_eq!(stringlike::from_str("str"), Some(StringLikeId::Str));
//! assert_eq!(stringlike::from_str("FString"), Some(StringLikeId::FString));
//! assert_eq!(stringlike::as_str(StringLikeId::Bytes), "bytes");
//! ```

use crate::lang::registry::{Example, RFC, RfcId, Since, Stability};

/// Stable identifier for string-like builtin types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringLikeId {
    Str,
    Bytes,
    FrozenStr,
    FrozenBytes,
    FString,
}

/// Metadata for a string-like builtin type.
#[derive(Debug, Clone, Copy)]
pub struct StringLikeInfo {
    pub id: StringLikeId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of string-like builtin types.
pub const STRING_LIKE_TYPES: &[StringLikeInfo] = &[
    info(
        StringLikeId::Str,
        "str",
        &[],
        "Builtin UTF-8 string type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        StringLikeId::Bytes,
        "bytes",
        &[],
        "Builtin byte buffer type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        StringLikeId::FrozenStr,
        "frozenstr",
        &["FrozenStr"],
        "Immutable/const-friendly string type.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        StringLikeId::FrozenBytes,
        "frozenbytes",
        &["FrozenBytes"],
        "Immutable/const-friendly bytes type.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        StringLikeId::FString,
        "fstring",
        &["FString"],
        "Formatted string result type.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a type name to a [`StringLikeId`].
///
/// ## Parameters
/// - `name`: Candidate type name (canonical or alias).
///
/// ## Returns
/// - `Some(StringLikeId)` if the spelling matches this registry.
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-insensitive ASCII**.
pub fn from_str(name: &str) -> Option<StringLikeId> {
    if let Some(t) = STRING_LIKE_TYPES
        .iter()
        .find(|t| t.canonical.eq_ignore_ascii_case(name))
    {
        return Some(t.id);
    }
    STRING_LIKE_TYPES
        .iter()
        .find(|t| t.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
        .map(|t| t.id)
}

/// Return the canonical spelling for a string-like builtin type.
///
/// ## Parameters
/// - `id`: String-like type identifier.
///
/// ## Returns
/// - The canonical spelling (e.g. `"str"`).
pub fn as_str(id: StringLikeId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a string-like builtin type.
///
/// ## Parameters
/// - `id`: String-like type identifier.
///
/// ## Returns
/// - The associated [`StringLikeInfo`] from [`STRING_LIKE_TYPES`].
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: StringLikeId) -> &'static StringLikeInfo {
    STRING_LIKE_TYPES
        .iter()
        .find(|t| t.id == id)
        .expect("INVARIANT: string-like info missing")
}

const fn info(
    id: StringLikeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> StringLikeInfo {
    StringLikeInfo {
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
