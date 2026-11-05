//! List method surface vocabulary.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a list method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListMethodId {
    Append,
    Extend,
    Clone,
    Pop,
    Contains,
    Swap,
    Reserve,
    ReserveExact,
    Remove,
    Count,
    Index,
}

pub type ListMethodInfo = LangItemInfo<ListMethodId>;

/// Registry of all list methods.
pub const LIST_METHODS: &[ListMethodInfo] = &[
    info(
        ListMethodId::Append,
        "append",
        &[],
        "Append an element to the end of the list.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Extend,
        "extend",
        &[],
        "Append all elements from another list.",
        RFC::_009,
        Since(0, 2),
    ),
    info(
        ListMethodId::Clone,
        "clone",
        &[],
        "Clone the list container and each element.",
        RFC::_009,
        Since(0, 3),
    ),
    info(
        ListMethodId::Pop,
        "pop",
        &[],
        "Remove and return the last element. On an empty list, panics with `IndexError: pop from empty list`.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Contains,
        "contains",
        &[],
        "Return true if the list contains a value.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Swap,
        "swap",
        &[],
        "Swap two elements by index.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Reserve,
        "reserve",
        &[],
        "Reserve capacity for at least N more elements.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::ReserveExact,
        "reserve_exact",
        &[],
        "Reserve capacity for exactly N more elements.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Remove,
        "remove",
        &[],
        "`remove(index) -> None`: remove the element at the given index.",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Count,
        "count",
        &[],
        "`items.count(value)` returns how many items equal `value`; `items.count()` with no argument is the iterator terminal and returns the number of items. Any other argument count is refused (`INCAN-T0001`).",
        RFC::_009,
        Since(0, 1),
    ),
    info(
        ListMethodId::Index,
        "index",
        &[],
        "Return the index of a value (or error if not found).",
        RFC::_009,
        Since(0, 1),
    ),
];

/// Resolve a list method spelling to its stable id.
pub fn from_str(name: &str) -> Option<ListMethodId> {
    super::from_str_impl(LIST_METHODS, name)
}

/// Return the canonical spelling for a list method.
pub fn as_str(id: ListMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for a list method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: ListMethodId) -> &'static ListMethodInfo {
    super::info_for_impl(LIST_METHODS, id, "list method info missing")
}

/// Whether calling the method changes the list it is called on: the methods that add, remove or reorder elements,
/// or change the list's capacity, do; `clone`, `contains`, `count` and `index` only read it.
///
/// The checker reads this one fact wherever it asks whether a call changes its receiver. The match is exhaustive,
/// so a new method states its effect.
pub const fn changes_receiver(id: ListMethodId) -> bool {
    match id {
        ListMethodId::Append
        | ListMethodId::Extend
        | ListMethodId::Pop
        | ListMethodId::Swap
        | ListMethodId::Reserve
        | ListMethodId::ReserveExact
        | ListMethodId::Remove => true,
        ListMethodId::Clone | ListMethodId::Contains | ListMethodId::Count | ListMethodId::Index => false,
    }
}

const fn info(
    id: ListMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> ListMethodInfo {
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
