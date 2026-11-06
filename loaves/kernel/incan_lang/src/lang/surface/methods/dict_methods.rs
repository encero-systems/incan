//! Dict method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a dict method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DictMethodId {
    Keys,
    Values,
    Get,
    Insert,
    /// `d.contains_key(key)` answers membership like `FrozenDict.contains_key` (#1668).
    ContainsKey,
}

pub type DictMethodInfo = LangItemInfo<DictMethodId>;

/// Registry of all dict methods.
pub const DICT_METHODS: &[DictMethodInfo] = &[
    info(
        DictMethodId::Keys,
        "keys",
        &[],
        "Return an iterable/list of keys.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        DictMethodId::Values,
        "values",
        &[],
        "Return an iterable/list of values.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        DictMethodId::Get,
        "get",
        &[],
        "`get(key)` returns `Some(value)` when the key is present and `None` otherwise; `get(key, default)` returns the value, or `default` when the key is absent. A kept lookup holds its own copy of the stored value, and one whose value type cannot be copied is refused with `INCAN-T0118`; see [Kept and in-place dict lookups](#kept-and-in-place-dict-lookups).",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        DictMethodId::Insert,
        "insert",
        &[],
        "Insert or overwrite a key/value pair.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        DictMethodId::ContainsKey,
        "contains_key",
        &[],
        "Return true if the dict contains a key.",
        RFC::_009,
        Since(0, 6),
    ),
];

/// Resolve a dict method spelling to its stable id.
pub fn from_str(name: &str) -> Option<DictMethodId> {
    super::from_str_impl(DICT_METHODS, name)
}

/// Return the canonical spelling for a dict method.
pub fn as_str(id: DictMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a dict method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: DictMethodId) -> &'static DictMethodInfo {
    super::info_for_impl(DICT_METHODS, id, "dict method info missing")
}

/// Whether calling the method changes the dict it is called on: `insert` does; `keys`, `values`, `get` and
/// `contains_key` only read it.
///
/// The checker reads this one fact wherever it asks whether a call changes its receiver. The match is exhaustive,
/// so a new method states its effect.
pub const fn changes_receiver(id: DictMethodId) -> bool {
    match id {
        DictMethodId::Insert => true,
        DictMethodId::Keys | DictMethodId::Values | DictMethodId::Get | DictMethodId::ContainsKey => false,
    }
}

const fn info(
    id: DictMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> DictMethodInfo {
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
