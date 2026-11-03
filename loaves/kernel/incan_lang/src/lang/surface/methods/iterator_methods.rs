//! Iterator protocol method surface vocabulary.

use super::LangItemInfo;
use crate::lang::registry::{RFC, Since, Stability};

/// Stable identifier for an RFC 088 iterator protocol method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IteratorMethodId {
    Iter,
    Map,
    Filter,
    Enumerate,
    Zip,
    Take,
    Skip,
    TakeWhile,
    SkipWhile,
    Chain,
    FlatMap,
    Batch,
    Collect,
    Count,
    Reduce,
    Fold,
    Any,
    All,
    Find,
    ForEach,
    Sum,
}

pub type IteratorMethodInfo = LangItemInfo<IteratorMethodId>;

pub const ITERATOR_METHODS: &[IteratorMethodInfo] = &[
    info(IteratorMethodId::Iter, "iter", "Create an iterator over an iterable."),
    info(IteratorMethodId::Map, "map", "Lazily transform iterator items."),
    info(
        IteratorMethodId::Filter,
        "filter",
        "Lazily keep items that match a predicate.",
    ),
    info(
        IteratorMethodId::Enumerate,
        "enumerate",
        "Yield each item with its zero-based index.",
    ),
    info(IteratorMethodId::Zip, "zip", "Pair items from two iterables."),
    info(
        IteratorMethodId::Take,
        "take",
        "Yield at most the requested number of items.",
    ),
    info(
        IteratorMethodId::Skip,
        "skip",
        "Discard at most the requested number of items.",
    ),
    info(
        IteratorMethodId::TakeWhile,
        "take_while",
        "Yield items until a predicate first returns false.",
    ),
    info(
        IteratorMethodId::SkipWhile,
        "skip_while",
        "Discard items while a predicate returns true.",
    ),
    info(
        IteratorMethodId::Chain,
        "chain",
        "Yield receiver items followed by another iterable.",
    ),
    info(
        IteratorMethodId::FlatMap,
        "flat_map",
        "Map items to iterables and flatten the result.",
    ),
    info(IteratorMethodId::Batch, "batch", "Yield fixed-size list batches."),
    info(IteratorMethodId::Collect, "collect", "Consume an iterator into a list."),
    info(
        IteratorMethodId::Count,
        "count",
        "Consume an iterator and return the item count.",
    ),
    info(
        IteratorMethodId::Reduce,
        "reduce",
        "Consume an iterator with an explicit accumulator.",
    ),
    info(
        IteratorMethodId::Fold,
        "fold",
        "Consume an iterator with an explicit accumulator.",
    ),
    info(
        IteratorMethodId::Any,
        "any",
        "Return whether any item satisfies a predicate.",
    ),
    info(
        IteratorMethodId::All,
        "all",
        "Return whether every item satisfies a predicate.",
    ),
    info(
        IteratorMethodId::Find,
        "find",
        "Return the first item satisfying a predicate.",
    ),
    info(
        IteratorMethodId::ForEach,
        "for_each",
        "Consume an iterator for side effects.",
    ),
    info(
        IteratorMethodId::Sum,
        "sum",
        "Consume an iterator and return the numeric sum.",
    ),
];

/// Resolve an iterator method spelling to its stable id.
pub fn from_str(name: &str) -> Option<IteratorMethodId> {
    super::from_str_impl(ITERATOR_METHODS, name)
}

/// Return the canonical spelling for an iterator method.
pub fn as_str(id: IteratorMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the full metadata entry for an iterator method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: IteratorMethodId) -> &'static IteratorMethodInfo {
    super::info_for_impl(ITERATOR_METHODS, id, "iterator method info missing")
}

/// Return whether this method consumes an iterator and therefore ends the iterator pipeline.
pub const fn is_terminal(id: IteratorMethodId) -> bool {
    matches!(
        id,
        IteratorMethodId::Collect
            | IteratorMethodId::Count
            | IteratorMethodId::Reduce
            | IteratorMethodId::Fold
            | IteratorMethodId::Any
            | IteratorMethodId::All
            | IteratorMethodId::Find
            | IteratorMethodId::ForEach
            | IteratorMethodId::Sum
    )
}

/// Return static metadata for this language-surface method.
const fn info(id: IteratorMethodId, canonical: &'static str, description: &'static str) -> IteratorMethodInfo {
    LangItemInfo {
        id,
        canonical,
        aliases: &[],
        description,
        introduced_in_rfc: RFC::_088,
        since: Since(0, 3),
        stability: Stability::Stable,
        examples: &[],
    }
}
