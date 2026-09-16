//! Prelude constructors / variant-like helpers (surface vocabulary).
//!
//! These are callable names in the global namespace that behave like constructors for common sum types
//! (e.g. `Ok(...)`, `Err(...)`, `Some(...)`).

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a surface constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstructorId {
    Ok,
    Err,
    Some,
    None,
}

/// Metadata for a surface constructor.
pub type ConstructorInfo = LangItemInfo<ConstructorId>;

/// Registry of all surface constructors.
pub const CONSTRUCTORS: &[ConstructorInfo] = &[
    info(
        ConstructorId::Ok,
        "Ok",
        &[],
        "Construct an `Ok(T)` variant (Result success).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ConstructorId::Err,
        "Err",
        &[],
        "Construct an `Err(E)` variant (Result failure).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ConstructorId::Some,
        "Some",
        &[],
        "Construct a `Some(T)` variant (Option present).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        ConstructorId::None,
        "None",
        &[],
        "Construct a `None` variant (Option absent).",
        RFC::_000,
        Since(0, 1),
    ),
];

pub fn from_str(name: &str) -> Option<ConstructorId> {
    if let Some(c) = CONSTRUCTORS.iter().find(|c| c.canonical == name) {
        return Some(c.id);
    }
    CONSTRUCTORS
        .iter()
        .find(|c| {
            let aliases: &[&str] = c.aliases;
            aliases.contains(&name)
        })
        .map(|c| c.id)
}

pub fn as_str(id: ConstructorId) -> &'static str {
    info_for(id).canonical
}

/// Return the metadata entry for a surface constructor.
///
/// The lookup is exhaustive over the closed enum, so adding a constructor requires updating this match at compile time.
pub fn info_for(id: ConstructorId) -> ConstructorInfo {
    match id {
        ConstructorId::Ok => CONSTRUCTORS[0],
        ConstructorId::Err => CONSTRUCTORS[1],
        ConstructorId::Some => CONSTRUCTORS[2],
        ConstructorId::None => CONSTRUCTORS[3],
    }
}

const fn info(
    id: ConstructorId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> ConstructorInfo {
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
