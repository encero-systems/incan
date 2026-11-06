//! FrozenBytes method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a frozen bytes method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FrozenBytesMethodId {
    Len,
    IsEmpty,
    /// Decode frozen UTF-8 bytes through the same public codec contract as runtime bytes.
    Decode,
}

pub type FrozenBytesMethodInfo = LangItemInfo<FrozenBytesMethodId>;

/// Registry of all frozen bytes methods.
pub const FROZEN_BYTES_METHODS: &[FrozenBytesMethodInfo] = &[
    info(
        FrozenBytesMethodId::Len,
        "len",
        &[],
        "Return the number of bytes.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenBytesMethodId::IsEmpty,
        "is_empty",
        &[],
        "Return true if the byte string is empty.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        FrozenBytesMethodId::Decode,
        "decode",
        &[],
        "Return the frozen bytes decoded as UTF-8 text through the public codec result.",
        RFC::_009,
        Since(0, 6),
    ),
];

/// Resolve a frozen bytes method spelling to its stable id.
pub fn from_str(name: &str) -> Option<FrozenBytesMethodId> {
    super::from_str_impl(FROZEN_BYTES_METHODS, name)
}

/// Return the canonical spelling for a frozen bytes method.
pub fn as_str(id: FrozenBytesMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a frozen bytes method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: FrozenBytesMethodId) -> &'static FrozenBytesMethodInfo {
    super::info_for_impl(FROZEN_BYTES_METHODS, id, "frozen bytes method info missing")
}

const fn info(
    id: FrozenBytesMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> FrozenBytesMethodInfo {
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
