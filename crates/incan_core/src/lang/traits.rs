//! Builtin trait vocabulary.
//!
//! This registry defines the canonical set of builtin trait names recognized by the compiler.
//! Callers should avoid hard-coding trait strings and instead use [`TraitId`] for identity.
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive** (trait names are case-sensitive).
//! - This module is vocabulary only (spellings + metadata), not trait semantics.

use super::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a builtin trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TraitId {
    Debug,
    Display,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Clone,
    Default,
    From,
    Into,
    TryFrom,
    TryInto,
    Iterator,
    IntoIterator,
    Error,
}

/// Metadata for a builtin trait.
pub type TraitInfo = LangItemInfo<TraitId>;

/// Registry of builtin traits.
pub const TRAITS: &[TraitInfo] = &[
    info(
        TraitId::Debug,
        "Debug",
        "Trait for debug formatting output.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Display,
        "Display",
        "Trait for user-facing string formatting.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Eq,
        "Eq",
        "Trait for equality comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::PartialEq,
        "PartialEq",
        "Trait for partial equality comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Ord,
        "Ord",
        "Trait for ordering comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::PartialOrd,
        "PartialOrd",
        "Trait for partial ordering comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Hash,
        "Hash",
        "Trait for hashing support.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Clone,
        "Clone",
        "Trait for cloning values.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Default,
        "Default",
        "Trait for default value construction.",
        RFC::_000,
        Since(0, 1),
    ),
    info(TraitId::From, "From", "Trait for conversions.", RFC::_000, Since(0, 1)),
    info(TraitId::Into, "Into", "Trait for conversions.", RFC::_000, Since(0, 1)),
    info(
        TraitId::TryFrom,
        "TryFrom",
        "Trait for fallible conversions.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::TryInto,
        "TryInto",
        "Trait for fallible conversions.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Iterator,
        "Iterator",
        "Trait for iterator behavior.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::IntoIterator,
        "IntoIterator",
        "Trait for conversion into iterators.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        TraitId::Error,
        "Error",
        "Trait for error-like values.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a spelling to a builtin trait identifier.
///
/// ## Notes
/// - Matching is **case-sensitive**.
pub fn from_str(name: &str) -> Option<TraitId> {
    TRAITS.iter().find(|t| t.canonical == name).map(|t| t.id)
}

/// Return the canonical spelling for a builtin trait.
pub fn as_str(id: TraitId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a builtin trait.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: TraitId) -> &'static TraitInfo {
    TRAITS
        .iter()
        .find(|t| t.id == id)
        .expect("INVARIANT: trait info missing")
}

const fn info(
    id: TraitId,
    canonical: &'static str,
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> TraitInfo {
    LangItemInfo {
        id,
        canonical,
        aliases: &[],
        description,
        introduced_in_rfc,
        since,
        stability: Stability::Stable,
        examples: &[],
    }
}
