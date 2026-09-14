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
    info_with_aliases(
        TraitId::From,
        "From",
        &["ConvertFrom"],
        "Trait for conversions.",
        RFC::_000,
        Since(0, 1),
    ),
    info_with_aliases(
        TraitId::Into,
        "Into",
        &["ConvertInto"],
        "Trait for conversions.",
        RFC::_000,
        Since(0, 1),
    ),
    info_with_aliases(
        TraitId::TryFrom,
        "TryFrom",
        &["ConvertTryFrom"],
        "Trait for fallible conversions.",
        RFC::_000,
        Since(0, 1),
    ),
    info_with_aliases(
        TraitId::TryInto,
        "TryInto",
        &["ConvertTryInto"],
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
    TRAITS
        .iter()
        .find(|t| t.canonical == name || t.aliases.contains(&name))
        .map(|t| t.id)
}

/// Resolve a bare, canonical source-qualified, or generated-provider spelling to a builtin trait identifier.
///
/// Qualified lookup uses each source-owned trait's registered module instead of accepting any `std.derives` path with a
/// familiar final segment. This keeps source and generated-provider identities exact while preventing an unrelated
/// declaration from acquiring builtin semantics by spelling alone.
pub fn from_qualified_str(name: &str) -> Option<TraitId> {
    if let Some(id) = from_str(name) {
        return Some(id);
    }

    let canonical = name
        .rsplit(['.', ':'])
        .find(|segment| !segment.is_empty())
        .and_then(from_str)?;
    let source_module = source_module(canonical)?;
    let source_path = format!("{source_module}.{}", as_str(canonical));
    if name == source_path {
        return Some(canonical);
    }

    let generated_module = generated_module(canonical)?;
    let generated_suffix = format!("::__incan_std::{generated_module}::{}", as_str(canonical));
    name.ends_with(&generated_suffix).then_some(canonical)
}

/// Return the canonical source module for a source-owned builtin trait.
#[must_use]
pub const fn source_module(id: TraitId) -> Option<&'static str> {
    match id {
        TraitId::Debug | TraitId::Display => Some("std.derives.string"),
        TraitId::Eq | TraitId::Ord | TraitId::Hash => Some("std.derives.comparison"),
        TraitId::Clone | TraitId::Default => Some("std.derives.copying"),
        TraitId::From | TraitId::Into | TraitId::TryFrom | TraitId::TryInto => Some("std.traits.convert"),
        TraitId::Error => Some("std.traits.error"),
        TraitId::Iterator | TraitId::Iterable | TraitId::Sum => Some("std.derives.collection"),
        TraitId::PartialEq | TraitId::PartialOrd | TraitId::IntoIterator | TraitId::Awaitable => None,
    }
}

/// Return the generated-provider module for a source-owned builtin trait.
#[must_use]
pub fn generated_module(id: TraitId) -> Option<String> {
    source_module(id).map(|module| module.strip_prefix("std.").unwrap_or(module).replace('.', "::"))
}

/// Return the canonical spelling for a builtin trait.
pub fn as_str(id: TraitId) -> &'static str {
    info_for(id).canonical
}

/// Return the canonical Rust paths under which inspected metadata may record a builtin trait implementation.
///
/// Rust exposes some builtin traits through more than one canonical namespace, such as a `core` definition with a
/// `std` re-export, and inspected Rust metadata records whichever path rust-analyzer resolves. Compiler layers that
/// match those records must take every admitted spelling from this registry rather than hardcoding path literals.
/// The list stays empty for traits interop does not yet need to recognize; extend it alongside new metadata
/// consumers.
#[must_use]
pub const fn rust_paths(id: TraitId) -> &'static [&'static str] {
    match id {
        TraitId::Default => &["core::default::Default", "std::default::Default"],
        TraitId::Clone => &["core::clone::Clone", "std::clone::Clone"],
        _ => &[],
    }
}

/// Return canonical source-declared method names for builtin traits whose method set is compiler-observed.
pub fn method_names(id: TraitId) -> &'static [&'static str] {
    match id {
        TraitId::Error => &["message", "source"],
        TraitId::From => &["from"],
        TraitId::Into => &["into"],
        TraitId::TryFrom => &["try_from"],
        TraitId::TryInto => &["try_into"],
        TraitId::Debug
        | TraitId::Display
        | TraitId::Eq
        | TraitId::PartialEq
        | TraitId::Ord
        | TraitId::PartialOrd
        | TraitId::Hash
        | TraitId::Clone
        | TraitId::Default
        | TraitId::Iterator
        | TraitId::IntoIterator
        | TraitId::Iterable
        | TraitId::Sum
        | TraitId::Awaitable => &[],
    }
}

/// Build a builtin trait metadata entry with explicit source aliases.
const fn info_with_aliases(
    id: TraitId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> TraitInfo {
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

#[cfg(test)]
mod tests {
    use super::{TraitId, from_qualified_str};

    #[test]
    fn qualified_lookup_uses_the_registered_source_owner() {
        assert_eq!(
            from_qualified_str("std.derives.collection.Iterator"),
            Some(TraitId::Iterator)
        );
        assert_eq!(
            from_qualified_str("stdlib_core::__incan_std::derives::collection::Iterator"),
            Some(TraitId::Iterator)
        );
        assert_eq!(from_qualified_str("std.traits.convert.From"), Some(TraitId::From));
        assert_eq!(
            from_qualified_str("stdlib_core::__incan_std::traits::convert::From"),
            Some(TraitId::From)
        );
        assert_eq!(from_qualified_str("std.derives.other.Iterator"), None);
        assert_eq!(from_qualified_str("RecordIterator"), None);
        assert_eq!(from_qualified_str("custom::Iterator"), None);
    }
}
