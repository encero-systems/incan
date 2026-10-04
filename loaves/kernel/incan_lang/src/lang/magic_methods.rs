//! Compiler-recognized magic (dunder) method spellings.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for magic methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MagicMethodId {
    Eq,
    Str,
    Iter,
    Next,
    ClassName,
    Fields,
    FieldValue,
    FieldItems,
    Slice,
}

/// Metadata entry for a magic method.
pub type MagicMethodInfo = LangItemInfo<MagicMethodId>;

/// Registry of recognized magic methods.
pub const MAGIC_METHODS: &[MagicMethodInfo] = &[
    info(
        MagicMethodId::Eq,
        "__eq__",
        &[],
        "Equality method.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        MagicMethodId::Str,
        "__str__",
        &[],
        "String conversion.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        MagicMethodId::Iter,
        "__iter__",
        &[],
        "Return the iteration state used by a for loop.",
        RFC::_068,
        Since(0, 3),
    ),
    info(
        MagicMethodId::Next,
        "__next__",
        &[],
        "Poll the next ordinary or fallible iteration item.",
        RFC::_068,
        Since(0, 3),
    ),
    info(
        MagicMethodId::ClassName,
        "__class_name__",
        &[],
        "Return class name string.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        MagicMethodId::Fields,
        "__fields__",
        &[],
        "Return reflected field list.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        MagicMethodId::FieldValue,
        "__field_value__",
        &[],
        "Return a reflected field value by runtime field name.",
        RFC::_030,
        Since(0, 3),
    ),
    info(
        MagicMethodId::FieldItems,
        "__field_items__",
        &[],
        "Return reflected field name/value pairs.",
        RFC::_030,
        Since(0, 3),
    ),
    info(
        MagicMethodId::Slice,
        "__slice__",
        &[],
        "Internal slice helper.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Resolve a magic method name to its stable id.
pub fn from_str(name: &str) -> Option<MagicMethodId> {
    if let Some(info) = MAGIC_METHODS.iter().find(|m| m.canonical == name) {
        return Some(info.id);
    }
    MAGIC_METHODS
        .iter()
        .find(|m| {
            let aliases: &[&str] = m.aliases;
            aliases.contains(&name)
        })
        .map(|m| m.id)
}

/// Return the canonical spelling for a magic method.
pub fn as_str(id: MagicMethodId) -> &'static str {
    info_for(id).canonical
}

/// Return the metadata entry for a magic method.
///
/// The lookup is exhaustive over the closed enum, so adding a magic method requires updating this match at compile
/// time.
pub fn info_for(id: MagicMethodId) -> MagicMethodInfo {
    match id {
        MagicMethodId::Eq => MAGIC_METHODS[0],
        MagicMethodId::Str => MAGIC_METHODS[1],
        MagicMethodId::Iter => MAGIC_METHODS[2],
        MagicMethodId::Next => MAGIC_METHODS[3],
        MagicMethodId::ClassName => MAGIC_METHODS[4],
        MagicMethodId::Fields => MAGIC_METHODS[5],
        MagicMethodId::FieldValue => MAGIC_METHODS[6],
        MagicMethodId::FieldItems => MAGIC_METHODS[7],
        MagicMethodId::Slice => MAGIC_METHODS[8],
    }
}

const fn info(
    id: MagicMethodId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> MagicMethodInfo {
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

/// Stable identifier for a comparison dunder: the source method that defines one comparison operator.
///
/// A comparison dunder is a method a type or trait declares, not a member the compiler provides, so these entries are
/// kept apart from [`MAGIC_METHODS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComparisonDunderId {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Metadata entry for a comparison dunder.
pub type ComparisonDunderInfo = LangItemInfo<ComparisonDunderId>;

/// Registry of the comparison dunders, one per comparison operator.
pub const COMPARISON_DUNDERS: &[ComparisonDunderInfo] = &[
    comparison(ComparisonDunderId::Eq, "__eq__", "Define `==`."),
    comparison(ComparisonDunderId::Ne, "__ne__", "Define `!=`."),
    comparison(ComparisonDunderId::Lt, "__lt__", "Define `<`."),
    comparison(ComparisonDunderId::Le, "__le__", "Define `<=`."),
    comparison(ComparisonDunderId::Gt, "__gt__", "Define `>`."),
    comparison(ComparisonDunderId::Ge, "__ge__", "Define `>=`."),
];

/// Resolve a comparison dunder name to its stable id.
pub fn comparison_from_str(name: &str) -> Option<ComparisonDunderId> {
    COMPARISON_DUNDERS
        .iter()
        .find(|info| info.canonical == name)
        .map(|info| info.id)
}

/// Return the canonical spelling for a comparison dunder.
pub fn comparison_as_str(id: ComparisonDunderId) -> &'static str {
    comparison_info_for(id).canonical
}

/// Return the metadata entry for a comparison dunder.
///
/// The lookup is exhaustive over the closed enum, so adding a comparison dunder requires updating this match at
/// compile time.
pub fn comparison_info_for(id: ComparisonDunderId) -> ComparisonDunderInfo {
    match id {
        ComparisonDunderId::Eq => COMPARISON_DUNDERS[0],
        ComparisonDunderId::Ne => COMPARISON_DUNDERS[1],
        ComparisonDunderId::Lt => COMPARISON_DUNDERS[2],
        ComparisonDunderId::Le => COMPARISON_DUNDERS[3],
        ComparisonDunderId::Gt => COMPARISON_DUNDERS[4],
        ComparisonDunderId::Ge => COMPARISON_DUNDERS[5],
    }
}

/// Build a comparison dunder entry: every comparison dunder is a stable RFC 000 spelling with no aliases.
const fn comparison(
    id: ComparisonDunderId,
    canonical: &'static str,
    description: &'static str,
) -> ComparisonDunderInfo {
    LangItemInfo {
        id,
        canonical,
        aliases: &[],
        description,
        introduced_in_rfc: RFC::_000,
        since: Since(0, 1),
        stability: Stability::Stable,
        examples: &[],
    }
}
