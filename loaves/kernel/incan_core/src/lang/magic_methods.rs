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
