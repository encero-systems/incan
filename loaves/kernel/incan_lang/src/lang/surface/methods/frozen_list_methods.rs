//! FrozenList method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a frozen list method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FrozenListMethodId {
    Len,
    IsEmpty,
}

pub type FrozenListMethodInfo = LangItemInfo<FrozenListMethodId>;

/// Registry of all frozen list methods.
pub const FROZEN_LIST_METHODS: &[FrozenListMethodInfo] = &[
    info(
        FrozenListMethodId::Len,
        "len",
        &[],
        "Return the number of elements.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenListMethodId::IsEmpty,
        "is_empty",
        &[],
        "Return true if the list is empty.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a frozen list method spelling to its stable id.
pub fn from_str(name: &str) -> Option<FrozenListMethodId> {
    super::from_str_impl(FROZEN_LIST_METHODS, name)
}

/// Return the canonical spelling for a frozen list method.
pub fn as_str(id: FrozenListMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a frozen list method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: FrozenListMethodId) -> &'static FrozenListMethodInfo {
    super::info_for_impl(FROZEN_LIST_METHODS, id, "frozen list method info missing")
}

const fn info(
    id: FrozenListMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> FrozenListMethodInfo {
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
