//! Compiler-recognized magic (dunder) method spellings.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for magic methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MagicMethodId {
    Eq,
    Str,
    ClassName,
    Fields,
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
        MagicMethodId::ClassName => MAGIC_METHODS[2],
        MagicMethodId::Fields => MAGIC_METHODS[3],
        MagicMethodId::Slice => MAGIC_METHODS[4],
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
