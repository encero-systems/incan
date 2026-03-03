//! Collection / generic-base builtin type vocabulary.
//!
//! This registry covers builtin collection and generic-base type names (e.g. `List`, `Dict`,
//! `Option`) and their accepted aliases.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**.
//! - Some types intentionally have lowercase aliases (`list`, `dict`, …) for user ergonomics.
//! - This module is vocabulary only (spellings + metadata), not type-system semantics.
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::types::collections::{self, CollectionTypeId};
//!
//! assert_eq!(collections::from_str("List"), Some(CollectionTypeId::List));
//! assert_eq!(collections::from_str("list"), Some(CollectionTypeId::List));
//! assert_eq!(collections::as_str(CollectionTypeId::Option), "Option");
//! ```

use crate::lang::registry::{Example, RFC, RfcId, Since, Stability};

/// Stable identifier for collection/generic-base builtin types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionTypeId {
    List,
    Dict,
    Set,
    Tuple,
    Option,
    Result,
    FrozenList,
    FrozenDict,
    FrozenSet,
}

/// Metadata for a collection/generic-base builtin type.
#[derive(Debug, Clone, Copy)]
pub struct CollectionTypeInfo {
    pub id: CollectionTypeId,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub introduced_in_rfc: RfcId,
    pub since: Since,
    pub stability: Stability,
    pub examples: &'static [Example],
}

/// Registry of collection/generic-base builtin types.
pub const COLLECTION_TYPES: &[CollectionTypeInfo] = &[
    info(
        CollectionTypeId::List,
        "List",
        &["list", "Vec"],
        "Growable list (generic sequence) type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::Dict,
        "Dict",
        &["dict", "HashMap"],
        "Key/value map type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::Set,
        "Set",
        &["set"],
        "Unordered set type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::Tuple,
        "Tuple",
        &["tuple"],
        "Fixed-length heterogeneous tuple type.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::Option,
        "Option",
        &["option"],
        "Optional value type (`Some`/`None`).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::Result,
        "Result",
        &["result"],
        "Result type (`Ok`/`Err`).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::FrozenList,
        "FrozenList",
        &["frozenlist"],
        "Immutable/const-friendly list type.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::FrozenDict,
        "FrozenDict",
        &["frozendict"],
        "Immutable/const-friendly dict type.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        CollectionTypeId::FrozenSet,
        "FrozenSet",
        &["frozenset"],
        "Immutable/const-friendly set type.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a type name to a [`CollectionTypeId`].
///
/// ## Parameters
/// - `name`: Candidate type name (canonical or alias).
///
/// ## Returns
/// - `Some(CollectionTypeId)` if the spelling matches this registry.
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-sensitive**.
pub fn from_str(name: &str) -> Option<CollectionTypeId> {
    if let Some(t) = COLLECTION_TYPES.iter().find(|t| t.canonical == name) {
        return Some(t.id);
    }
    COLLECTION_TYPES
        .iter()
        .find(|t| {
            let aliases: &[&str] = t.aliases;
            aliases.contains(&name)
        })
        .map(|t| t.id)
}

/// Return the canonical spelling for a collection/generic-base builtin type.
///
/// ## Parameters
/// - `id`: Collection type identifier.
///
/// ## Returns
/// - The canonical spelling (e.g. `"List"`).
pub fn as_str(id: CollectionTypeId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a collection/generic-base builtin type.
///
/// ## Parameters
/// - `id`: Collection type identifier.
///
/// ## Returns
/// - The associated [`CollectionTypeInfo`] from [`COLLECTION_TYPES`].
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: CollectionTypeId) -> &'static CollectionTypeInfo {
    COLLECTION_TYPES
        .iter()
        .find(|t| t.id == id)
        .expect("INVARIANT: collection type info missing")
}

const fn info(
    id: CollectionTypeId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> CollectionTypeInfo {
    CollectionTypeInfo {
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
