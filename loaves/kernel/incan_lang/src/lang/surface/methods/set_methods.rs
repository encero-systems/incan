//! Set method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a set method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetMethodId {
    Add,
    Contains,
}

pub type SetMethodInfo = LangItemInfo<SetMethodId>;

/// Registry of all set methods.
pub const SET_METHODS: &[SetMethodInfo] = &[
    info(
        SetMethodId::Add,
        "add",
        &[],
        "Add a value to the set.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        SetMethodId::Contains,
        "contains",
        &[],
        "Return true if the set contains a value.",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a set method spelling to its stable id.
pub fn from_str(name: &str) -> Option<SetMethodId> {
    super::from_str_impl(SET_METHODS, name)
}

/// Return the canonical spelling for a set method.
pub fn as_str(id: SetMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a set method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: SetMethodId) -> &'static SetMethodInfo {
    super::info_for_impl(SET_METHODS, id, "set method info missing")
}

/// Whether calling the method changes the set it is called on: `add` does, `contains` only reads it.
///
/// The checker reads this one fact wherever it asks whether a call changes its receiver. The match is exhaustive,
/// so a new method states its effect.
pub const fn changes_receiver(id: SetMethodId) -> bool {
    match id {
        SetMethodId::Add => true,
        SetMethodId::Contains => false,
    }
}

const fn info(
    id: SetMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> SetMethodInfo {
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
