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
    Iterable,
    Sum,
    Awaitable,
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
    info(
        TraitId::Iterable,
        "Iterable",
        "Trait for values that produce iterators.",
        RFC::_006,
        Since(0, 3),
    ),
    info(
        TraitId::Sum,
        "Sum",
        "Trait for values that can be produced by summing iterator items.",
        RFC::_088,
        Since(0, 3),
    ),
    info(
        TraitId::Awaitable,
        "Awaitable",
        "Trait for values that can be awaited to produce a value.",
        "RFC 039",
        Since(0, 3),
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
/// The lookup is exhaustive over the closed enum, so adding a trait requires updating this match at compile time.
pub fn info_for(id: TraitId) -> TraitInfo {
    match id {
        TraitId::Debug => TRAITS[0],
        TraitId::Display => TRAITS[1],
        TraitId::Eq => TRAITS[2],
        TraitId::PartialEq => TRAITS[3],
        TraitId::Ord => TRAITS[4],
        TraitId::PartialOrd => TRAITS[5],
        TraitId::Hash => TRAITS[6],
        TraitId::Clone => TRAITS[7],
        TraitId::Default => TRAITS[8],
        TraitId::From => TRAITS[9],
        TraitId::Into => TRAITS[10],
        TraitId::TryFrom => TRAITS[11],
        TraitId::TryInto => TRAITS[12],
        TraitId::Iterator => TRAITS[13],
        TraitId::IntoIterator => TRAITS[14],
        TraitId::Error => TRAITS[15],
        TraitId::Iterable => TRAITS[16],
        TraitId::Sum => TRAITS[17],
        TraitId::Awaitable => TRAITS[18],
    }
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
