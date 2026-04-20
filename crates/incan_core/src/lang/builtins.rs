//! Builtin function vocabulary.
//!
//! This module defines the canonical set of builtin functions recognized by the compiler.
//! Callers should use the stable identifier [`BuiltinFnId`] for identity and query spellings and other metadata
//! through [`BuiltinFnInfo`] / [`BUILTIN_FUNCTIONS`].
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**.
//! - Aliases exist for backwards compatibility and ergonomics (e.g. `"println"` is accepted as an alias for `"print"`).
//!
//! ## Examples
//! ```rust
//! use incan_core::lang::builtins::{self, BuiltinFnId};
//!
//! assert_eq!(builtins::from_str("print"), Some(BuiltinFnId::Print));
//! assert_eq!(builtins::from_str("println"), Some(BuiltinFnId::Print));
//! assert_eq!(builtins::as_str(BuiltinFnId::Print), "print");
//! ```

use super::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a builtin function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinFnId {
    Print,
    Len,
    Sum,
    Min,
    Max,
    Str,
    Int,
    Float,
    Bool,
    Abs,
    Range,
    Enumerate,
    Zip,
    Sorted,
    ReadFile,
    WriteFile,
    JsonStringify,
}

/// Metadata for a builtin function.
///
/// ## Notes
/// - `canonical` is the spelling used in docs and (usually) the preferred user-facing name.
/// - `aliases` are additional spellings accepted by the compiler.
pub type BuiltinFnInfo = LangItemInfo<BuiltinFnId>;

/// Registry of all builtin functions.
pub const BUILTIN_FUNCTIONS: &[BuiltinFnInfo] = &[
    info(
        BuiltinFnId::Print,
        "print",
        &["println"],
        "Print values to stdout.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Len,
        "len",
        &[],
        "Return the length of a collection/string.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Sum,
        "sum",
        &[],
        "Sum a numeric iterable/collection.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Min,
        "min",
        &[],
        "Return the minimum element of a collection.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Max,
        "max",
        &[],
        "Return the maximum element of a collection.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Str,
        "str",
        &[],
        "Convert a value to a string.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Int,
        "int",
        &[],
        "Convert a value to an integer.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Float,
        "float",
        &[],
        "Convert a value to a float.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Bool,
        "bool",
        &[],
        "Convert a value to a boolean.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Abs,
        "abs",
        &[],
        "Absolute value (numeric).",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Range,
        "range",
        &[],
        "Create a range of integers.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Enumerate,
        "enumerate",
        &[],
        "Enumerate an iterable into (index, value) pairs.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Zip,
        "zip",
        &[],
        "Zip iterables element-wise into tuples.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::Sorted,
        "sorted",
        &[],
        "Return a sorted copy of a collection.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::ReadFile,
        "read_file",
        &[],
        "Read a file from disk into a string/bytes.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::WriteFile,
        "write_file",
        &[],
        "Write a string/bytes to a file on disk.",
        RFC::_000,
        Since(0, 1),
    ),
    info(
        BuiltinFnId::JsonStringify,
        "json_stringify",
        &[],
        "Serialize a value to JSON.",
        RFC::_000,
        Since(0, 1),
    ),
];

/// Return the canonical spelling for a builtin function.
///
/// ## Parameters
/// - `id`: Builtin function identifier.
///
/// ## Returns
/// - The canonical spelling (e.g. `"print"`).
///
/// ## Examples
/// ```rust
/// use incan_core::lang::builtins::{self, BuiltinFnId};
///
/// assert_eq!(builtins::as_str(BuiltinFnId::Print), "print");
/// ```
pub fn as_str(id: BuiltinFnId) -> &'static str {
    info_for(id).canonical
}

/// Return the accepted aliases for a builtin function.
///
/// ## Parameters
/// - `id`: Builtin function identifier.
///
/// ## Returns
/// - A slice of alias spellings.
pub fn aliases(id: BuiltinFnId) -> &'static [&'static str] {
    info_for(id).aliases
}

/// Return the full metadata entry for a builtin function.
///
/// ## Parameters
/// - `id`: Builtin function identifier.
///
/// ## Returns
/// - The associated [`BuiltinFnInfo`] copied from [`BUILTIN_FUNCTIONS`].
///
/// The lookup is exhaustive over the closed enum, so adding a builtin requires updating this match at compile time.
pub fn info_for(id: BuiltinFnId) -> BuiltinFnInfo {
    match id {
        BuiltinFnId::Print => BUILTIN_FUNCTIONS[0],
        BuiltinFnId::Len => BUILTIN_FUNCTIONS[1],
        BuiltinFnId::Sum => BUILTIN_FUNCTIONS[2],
        BuiltinFnId::Min => BUILTIN_FUNCTIONS[3],
        BuiltinFnId::Max => BUILTIN_FUNCTIONS[4],
        BuiltinFnId::Str => BUILTIN_FUNCTIONS[5],
        BuiltinFnId::Int => BUILTIN_FUNCTIONS[6],
        BuiltinFnId::Float => BUILTIN_FUNCTIONS[7],
        BuiltinFnId::Bool => BUILTIN_FUNCTIONS[8],
        BuiltinFnId::Abs => BUILTIN_FUNCTIONS[9],
        BuiltinFnId::Range => BUILTIN_FUNCTIONS[10],
        BuiltinFnId::Enumerate => BUILTIN_FUNCTIONS[11],
        BuiltinFnId::Zip => BUILTIN_FUNCTIONS[12],
        BuiltinFnId::Sorted => BUILTIN_FUNCTIONS[13],
        BuiltinFnId::ReadFile => BUILTIN_FUNCTIONS[14],
        BuiltinFnId::WriteFile => BUILTIN_FUNCTIONS[15],
        BuiltinFnId::JsonStringify => BUILTIN_FUNCTIONS[16],
    }
}

/// Resolve a spelling to a builtin function identifier.
///
/// ## Parameters
/// - `name`: Candidate builtin name (canonical or alias).
///
/// ## Returns
/// - `Some(BuiltinFnId)` if `name` matches a canonical spelling or alias.
/// - `None` otherwise.
///
/// ## Notes
/// - Matching is **case-sensitive**.
pub fn from_str(name: &str) -> Option<BuiltinFnId> {
    if let Some(b) = BUILTIN_FUNCTIONS.iter().find(|b| b.canonical == name) {
        return Some(b.id);
    }
    BUILTIN_FUNCTIONS
        .iter()
        .find(|b| {
            let aliases: &[&str] = b.aliases;
            aliases.contains(&name)
        })
        .map(|b| b.id)
}

const fn info(
    id: BuiltinFnId,
    canonical: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    introduced_in_rfc: RfcId,
    since: Since,
) -> BuiltinFnInfo {
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
