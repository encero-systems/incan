//! FrozenDict method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a frozen dict method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrozenDictMethodId {
    Len,
    IsEmpty,
    ContainsKey,
}

pub type FrozenDictMethodInfo = LangItemInfo<FrozenDictMethodId>;

/// Registry of all frozen dict methods.
pub const FROZEN_DICT_METHODS: &[FrozenDictMethodInfo] = &[
    info(
        FrozenDictMethodId::Len,
        "len",
        &[],
        "Return the number of entries.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenDictMethodId::IsEmpty,
        "is_empty",
        &[],
        "Return true if the dict is empty.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenDictMethodId::ContainsKey,
        "contains_key",
        &[],
        "Return true if the dict contains a key.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a frozen dict method spelling to its stable id.
pub fn from_str(name: &str) -> Option<FrozenDictMethodId> {
    super::from_str_impl(FROZEN_DICT_METHODS, name)
}

/// Return the canonical spelling for a frozen dict method.
pub fn as_str(id: FrozenDictMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a frozen dict method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: FrozenDictMethodId) -> &'static FrozenDictMethodInfo {
    super::info_for_impl(FROZEN_DICT_METHODS, id, "frozen dict method info missing")
}

const fn info(
    id: FrozenDictMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> FrozenDictMethodInfo {
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
