//! FrozenSet method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a frozen set method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FrozenSetMethodId {
    Len,
    IsEmpty,
    Contains,
}

pub type FrozenSetMethodInfo = LangItemInfo<FrozenSetMethodId>;

/// Registry of all frozen set methods.
pub const FROZEN_SET_METHODS: &[FrozenSetMethodInfo] = &[
    info(
        FrozenSetMethodId::Len,
        "len",
        &[],
        "Return the number of elements.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenSetMethodId::IsEmpty,
        "is_empty",
        &[],
        "Return true if the set is empty.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenSetMethodId::Contains,
        "contains",
        &[],
        "Return true if the set contains a value.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a frozen set method spelling to its stable id.
pub fn from_str(name: &str) -> Option<FrozenSetMethodId> {
    super::from_str_impl(FROZEN_SET_METHODS, name)
}

/// Return the canonical spelling for a frozen set method.
pub fn as_str(id: FrozenSetMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a frozen set method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: FrozenSetMethodId) -> &'static FrozenSetMethodInfo {
    super::info_for_impl(FROZEN_SET_METHODS, id, "frozen set method info missing")
}

const fn info(
    id: FrozenSetMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> FrozenSetMethodInfo {
    LangItemInfo {
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
