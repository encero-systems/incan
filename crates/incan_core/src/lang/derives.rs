//! Builtin derive vocabulary.
//!
//! This registry defines the canonical set of derive names accepted by the compiler via `@derive(...)`.
//! Callers should avoid hard-coding derive strings and instead use [`DeriveId`] for identity.
//!
//! ## Notes
//! - Derives are treated as *language vocabulary* (like keywords/operators) even though they map to Rust traits/derive
//!   macros under the hood.
//! - Matching is **case-sensitive** (Rust trait names are case-sensitive).

use super::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a builtin derive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeriveId {
    // String representation
    Debug,
    Display,

    // Comparison
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,

    // Copying / defaults
    Clone,
    Copy,
    Default,

    // Validation
    Validate,
}

/// Metadata for a builtin derive.
pub type DeriveInfo = LangItemInfo<DeriveId>;

/// Registry of all builtin derives accepted by the compiler.
pub const DERIVES: &[DeriveInfo] = &[
    info(
        DeriveId::Debug,
        "Debug",
        "Derive Rust-style debug formatting.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Display,
        "Display",
        "Derive user-facing string formatting.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Eq,
        "Eq",
        "Derive equality comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::PartialEq,
        "PartialEq",
        "Derive partial equality comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Ord,
        "Ord",
        "Derive ordering comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::PartialOrd,
        "PartialOrd",
        "Derive partial ordering comparisons.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Hash,
        "Hash",
        "Derive hashing support (for map/set keys).",
        RFC::_000,
        Since(0, 1),
    ),
    info(DeriveId::Clone, "Clone", "Derive deep cloning.", RFC::_000, Since(0, 1)),
    info(
        DeriveId::Copy,
        "Copy",
        "Derive copy semantics for simple value types.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Default,
        "Default",
        "Derive a default value constructor.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        DeriveId::Validate,
        "Validate",
        "Enable validated construction via `TypeName.new(...)` and require a `validate(self) -> Result[Self, E]` method.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a spelling to a [`DeriveId`].
///
/// ## Parameters
/// - `name`: Candidate derive name.
///
/// ## Returns
/// - `Some(DeriveId)` if `name` matches a known derive.
/// - `None` otherwise.
pub fn from_str(name: &str) -> Option<DeriveId> {
    DERIVES.iter().find(|d| d.canonical == name).map(|d| d.id)
}

/// Return the canonical spelling for a derive.
pub fn as_str(id: DeriveId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a derive.
///
/// The lookup is exhaustive over the closed enum, so adding a derive requires updating this match at compile time.
pub fn info_for(id: DeriveId) -> DeriveInfo {
    match id {
        DeriveId::Debug => DERIVES[0],
        DeriveId::Display => DERIVES[1],
        DeriveId::Eq => DERIVES[2],
        DeriveId::PartialEq => DERIVES[3],
        DeriveId::Ord => DERIVES[4],
        DeriveId::PartialOrd => DERIVES[5],
        DeriveId::Hash => DERIVES[6],
        DeriveId::Clone => DERIVES[7],
        DeriveId::Copy => DERIVES[8],
        DeriveId::Default => DERIVES[9],
        DeriveId::Validate => DERIVES[10],
    }
}

const fn info(
    id: DeriveId,
    canonical: &'static str,
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> DeriveInfo {
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
