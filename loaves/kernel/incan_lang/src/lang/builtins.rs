//! Builtin function vocabulary.
//!
//! This module defines the canonical set of builtin functions recognized by the compiler. Callers should use the stable
//! identifier [`BuiltinFnId`] for identity and query spellings and other metadata through [`BuiltinFnInfo`] /
//! [`BUILTIN_FUNCTIONS`].
//!
//! ## Notes
//! - Lookup via [`from_str`] is **case-sensitive**.
//! - Aliases exist for backwards compatibility and ergonomics (e.g. `"println"` is accepted as an alias for `"print"`).
//!
//! ## Examples
//! ```rust
//! use incan_lang::lang::builtins::{self, BuiltinFnId};
//!
//! assert_eq!(builtins::from_str("print"), Some(BuiltinFnId::Print));
//! assert_eq!(builtins::from_str("println"), Some(BuiltinFnId::Print));
//! assert_eq!(builtins::as_str(BuiltinFnId::Print), "print");
//! ```

use super::registry::{LangItemInfo, RFC, RfcId, Since, Stability};
use super::surface::{
    dict_methods::DictMethodId, frozen_dict_methods::FrozenDictMethodId, frozen_list_methods::FrozenListMethodId,
    frozen_set_methods::FrozenSetMethodId, list_methods::ListMethodId, set_methods::SetMethodId,
};

/// Stable identifier for a builtin function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    IsInstance,
}

/// Stable checker-selected identity for a compiler-owned builtin method.
///
/// The receiver-family variants preserve distinctions that a method spelling alone cannot carry. This enum is the
/// semantic operation key transported through Body IR; canonical symbol identity remains separate provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BuiltinMethodId {
    /// Mutable and query operations on `List[T]`.
    List(ListMethodId),
    /// Borrow or consume a `List[T]` as its lazy iterator state.
    ListIter,
    /// Mutable and query operations on `Dict[K, V]`.
    Dict(DictMethodId),
    /// Mutable and query operations on `Set[T]`.
    Set(SetMethodId),
    /// Query operations on `FrozenList[T]`.
    FrozenList(FrozenListMethodId),
    /// Query operations on `FrozenDict[K, V]`.
    FrozenDict(FrozenDictMethodId),
    /// Query operations on `FrozenSet[T]`.
    FrozenSet(FrozenSetMethodId),
}

impl BuiltinMethodId {
    /// Return the compiler-owned receiver family used in canonical provenance.
    pub fn owner(self) -> &'static str {
        use super::types::collections::{CollectionTypeId, as_str};

        match self {
            Self::List(_) | Self::ListIter => as_str(CollectionTypeId::List),
            Self::Dict(_) => as_str(CollectionTypeId::Dict),
            Self::Set(_) => as_str(CollectionTypeId::Set),
            Self::FrozenList(_) => as_str(CollectionTypeId::FrozenList),
            Self::FrozenDict(_) => as_str(CollectionTypeId::FrozenDict),
            Self::FrozenSet(_) => as_str(CollectionTypeId::FrozenSet),
        }
    }

    /// Return the canonical member spelling used only to validate retained provenance.
    pub fn member(self) -> &'static str {
        match self {
            Self::List(method) => super::surface::list_methods::as_str(method),
            Self::ListIter => {
                super::surface::iterator_methods::as_str(super::surface::iterator_methods::IteratorMethodId::Iter)
            }
            Self::Dict(method) => super::surface::dict_methods::as_str(method),
            Self::Set(method) => super::surface::set_methods::as_str(method),
            Self::FrozenList(method) => super::surface::frozen_list_methods::as_str(method),
            Self::FrozenDict(method) => super::surface::frozen_dict_methods::as_str(method),
            Self::FrozenSet(method) => super::surface::frozen_set_methods::as_str(method),
        }
    }

    /// Return whether this operation requires exclusive receiver access.
    pub const fn changes_receiver(self) -> bool {
        match self {
            Self::List(method) => super::surface::list_methods::changes_receiver(method),
            Self::Dict(method) => super::surface::dict_methods::changes_receiver(method),
            Self::Set(method) => super::surface::set_methods::changes_receiver(method),
            Self::ListIter | Self::FrozenList(_) | Self::FrozenDict(_) | Self::FrozenSet(_) => false,
        }
    }
}

/// Resolve the canonical owner and member stored by a checked-body record to its stable builtin method identity.
pub fn method_from_parts(owner: &str, member: &str) -> Option<BuiltinMethodId> {
    use super::types::collections::{CollectionTypeId, from_str};

    match from_str(owner)? {
        CollectionTypeId::List => match super::surface::list_methods::from_str(member) {
            Some(method) => Some(BuiltinMethodId::List(method)),
            None if super::surface::iterator_methods::from_str(member)
                == Some(super::surface::iterator_methods::IteratorMethodId::Iter) =>
            {
                Some(BuiltinMethodId::ListIter)
            }
            None => None,
        },
        CollectionTypeId::Dict => super::surface::dict_methods::from_str(member).map(BuiltinMethodId::Dict),
        CollectionTypeId::Set => super::surface::set_methods::from_str(member).map(BuiltinMethodId::Set),
        CollectionTypeId::FrozenList => {
            super::surface::frozen_list_methods::from_str(member).map(BuiltinMethodId::FrozenList)
        }
        CollectionTypeId::FrozenDict => {
            super::surface::frozen_dict_methods::from_str(member).map(BuiltinMethodId::FrozenDict)
        }
        CollectionTypeId::FrozenSet => {
            super::surface::frozen_set_methods::from_str(member).map(BuiltinMethodId::FrozenSet)
        }
        _ => None,
    }
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
    info(
        BuiltinFnId::IsInstance,
        "isinstance",
        &[],
        "Test whether a value is an instance of a type and narrow union branches.",
        RFC::_029,
        Since(0, 3),
    ),
];

/// Builtin identities whose canonical spelling and registered aliases cannot be replaced by source bindings.
///
/// This is intentionally narrower than [`BUILTIN_FUNCTIONS`]: adding a builtin does not reserve its spelling.
const PROTECTED_BINDING_BUILTINS: &[BuiltinFnId] = &[BuiltinFnId::Print];

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
/// use incan_lang::lang::builtins::{self, BuiltinFnId};
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
        BuiltinFnId::IsInstance => BUILTIN_FUNCTIONS[17],
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

/// Return the protected builtin identity for a source binding spelling.
///
/// Protected bindings stay globally available: source declarations, imports, and lexical bindings cannot replace
/// their canonical spelling or any registered alias. The policy is intentionally narrower than the builtin registry;
/// adding a builtin does not protect its spelling unless its stable identifier is added here.
///
/// ## Parameters
/// - `name`: Candidate source binding spelling.
///
/// ## Returns
/// - `Some(BuiltinFnId)` when `name` names a protected builtin or one of its registry aliases.
/// - `None` for ordinary source bindings and unprotected builtins.
pub fn protected_binding_builtin(name: &str) -> Option<BuiltinFnId> {
    let builtin = from_str(name)?;
    PROTECTED_BINDING_BUILTINS.contains(&builtin).then_some(builtin)
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

#[cfg(test)]
mod tests {
    use super::{BuiltinFnId, BuiltinMethodId, method_from_parts, protected_binding_builtin};
    use crate::lang::surface::{list_methods::ListMethodId, set_methods::SetMethodId};

    /// Protect function aliases and collection-method effects through their typed registry identities.
    #[test]
    fn builtin_registry_owns_aliases_and_method_effects() {
        assert_eq!(protected_binding_builtin("print"), Some(BuiltinFnId::Print));
        assert_eq!(protected_binding_builtin("println"), Some(BuiltinFnId::Print));
        assert_eq!(protected_binding_builtin("len"), None);
        assert!(BuiltinMethodId::List(ListMethodId::Append).changes_receiver());
        assert!(!BuiltinMethodId::List(ListMethodId::Contains).changes_receiver());
        assert!(BuiltinMethodId::Set(SetMethodId::Add).changes_receiver());
        let operation = BuiltinMethodId::ListIter;
        assert_eq!(
            method_from_parts(operation.owner(), operation.member()),
            Some(operation)
        );
    }
}
