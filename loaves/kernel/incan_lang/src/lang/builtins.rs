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
    bytes_methods::BytesMethodId, clone_methods::CloneMethodId, dict_methods::DictMethodId,
    float_methods::FloatMethodId, frozen_bytes_methods::FrozenBytesMethodId, frozen_dict_methods::FrozenDictMethodId,
    frozen_list_methods::FrozenListMethodId, frozen_set_methods::FrozenSetMethodId, iterator_methods::IteratorMethodId,
    list_methods::ListMethodId, option_methods::OptionMethodId, result_methods::ResultMethodId,
    set_methods::SetMethodId, string_methods::StringMethodId,
};
use super::types::numerics::{IntegerHelperFamily, IntegerHelperOperation, NumericResizeMethodId, NumericTypeId};

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
    /// Runtime text operations on `str`.
    Str(StringMethodId),
    /// Frozen text operations on `frozenstr`.
    FrozenStr(StringMethodId),
    /// Runtime byte-buffer operations on `bytes`.
    Bytes(BytesMethodId),
    /// Frozen byte operations on `frozenbytes`.
    FrozenBytes(FrozenBytesMethodId),
    /// Protocol operations on an opaque `Iterator[T]` value.
    Iterator(IteratorMethodId),
    /// Adapter and consumer operations on a concrete `Generator[T]` value.
    Generator(IteratorMethodId),
    /// Payload and predicate operations on `Option[T]`.
    Option(OptionMethodId),
    /// Payload, predicate, and combinator operations on `Result[T, E]`.
    Result(ResultMethodId),
    /// Compiler-owned generic Clone operation.
    Clone(CloneMethodId),
    /// Native float operation selected for one concrete numeric receiver.
    Float(NumericTypeId, FloatMethodId),
    /// Integer overflow helper selected for one concrete numeric receiver.
    IntegerHelper(NumericTypeId, IntegerHelperFamily, IntegerHelperOperation),
    /// Numeric resize policy selected for one concrete numeric receiver.
    NumericResize(NumericTypeId, NumericResizeMethodId),
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
            Self::Str(_) => super::types::stringlike::as_str(super::types::stringlike::StringLikeId::Str),
            Self::FrozenStr(_) => super::types::stringlike::as_str(super::types::stringlike::StringLikeId::FrozenStr),
            Self::Bytes(_) => super::types::stringlike::as_str(super::types::stringlike::StringLikeId::Bytes),
            Self::FrozenBytes(_) => {
                super::types::stringlike::as_str(super::types::stringlike::StringLikeId::FrozenBytes)
            }
            Self::Iterator(_) => super::traits::as_str(super::traits::TraitId::Iterator),
            Self::Generator(_) => super::types::collections::as_str(CollectionTypeId::Generator),
            Self::Option(_) => super::types::collections::as_str(CollectionTypeId::Option),
            Self::Result(_) => super::types::collections::as_str(CollectionTypeId::Result),
            Self::Clone(_) => super::traits::as_str(super::traits::TraitId::Clone),
            Self::Float(owner, _) | Self::IntegerHelper(owner, _, _) | Self::NumericResize(owner, _) => {
                super::types::numerics::as_str(owner)
            }
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
            Self::Str(method) | Self::FrozenStr(method) => super::surface::string_methods::as_str(method),
            Self::Bytes(method) => super::surface::bytes_methods::as_str(method),
            Self::FrozenBytes(method) => super::surface::frozen_bytes_methods::as_str(method),
            Self::Iterator(method) | Self::Generator(method) => super::surface::iterator_methods::as_str(method),
            Self::Option(method) => super::surface::option_methods::as_str(method),
            Self::Result(method) => super::surface::result_methods::as_str(method),
            Self::Clone(method) => super::surface::clone_methods::as_str(method),
            Self::Float(_, method) => super::surface::float_methods::as_str(method),
            Self::IntegerHelper(_, family, operation) => {
                super::types::numerics::integer_helper_as_str(family, operation)
            }
            Self::NumericResize(_, method) => super::types::numerics::resize_method_as_str(method),
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
            Self::Str(_)
            | Self::FrozenStr(_)
            | Self::Bytes(_)
            | Self::FrozenBytes(_)
            | Self::Iterator(_)
            | Self::Generator(_)
            | Self::Option(_)
            | Self::Result(_)
            | Self::Clone(_)
            | Self::Float(_, _)
            | Self::IntegerHelper(_, _, _)
            | Self::NumericResize(_, _)
            | Self::ListIter
            | Self::FrozenList(_)
            | Self::FrozenDict(_)
            | Self::FrozenSet(_) => false,
        }
    }
}

/// Resolve the canonical owner and member stored by a checked-body record to its stable builtin method identity.
pub fn method_from_parts(owner: &str, member: &str) -> Option<BuiltinMethodId> {
    if owner == super::traits::as_str(super::traits::TraitId::Iterator) {
        return super::surface::iterator_methods::from_str(member).map(BuiltinMethodId::Iterator);
    }
    if owner == super::traits::as_str(super::traits::TraitId::Clone) {
        return super::surface::clone_methods::from_str(member).map(BuiltinMethodId::Clone);
    }

    use super::types::stringlike::{StringLikeId, from_str as stringlike_from_str};

    match stringlike_from_str(owner) {
        Some(StringLikeId::Str) => {
            if member == super::surface::string_methods::as_str(StringMethodId::Slice) {
                return Some(BuiltinMethodId::Str(StringMethodId::Slice));
            }
            return super::surface::string_methods::from_str(member).map(BuiltinMethodId::Str);
        }
        Some(StringLikeId::FrozenStr) => {
            return super::surface::string_methods::from_str(member).map(BuiltinMethodId::FrozenStr);
        }
        Some(StringLikeId::Bytes) => {
            return super::surface::bytes_methods::from_str(member).map(BuiltinMethodId::Bytes);
        }
        Some(StringLikeId::FrozenBytes) => {
            return super::surface::frozen_bytes_methods::from_str(member).map(BuiltinMethodId::FrozenBytes);
        }
        Some(StringLikeId::FString) => return None,
        None => {}
    }

    if let Some(owner) = super::types::numerics::from_str(owner) {
        if let Some(method) = super::types::numerics::resize_method_from_str(member) {
            return Some(BuiltinMethodId::NumericResize(owner, method));
        }
        if let Some(method) = super::surface::float_methods::from_str(member) {
            return super::types::numerics::is_binary_float(owner).then_some(BuiltinMethodId::Float(owner, method));
        }
        if let Some(helper) = super::types::numerics::integer_helper_from_str(member) {
            return super::types::numerics::supports_integer_helper(owner, helper)
                .then_some(BuiltinMethodId::IntegerHelper(owner, helper.family, helper.operation));
        }
        return None;
    }

    use super::types::collections::{CollectionTypeId, from_str};

    match from_str(owner)? {
        CollectionTypeId::Generator => {
            super::surface::iterator_methods::from_str(member).map(BuiltinMethodId::Generator)
        }
        CollectionTypeId::Option => super::surface::option_methods::from_str(member).map(BuiltinMethodId::Option),
        CollectionTypeId::Result => super::surface::result_methods::from_str(member).map(BuiltinMethodId::Result),
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
    use crate::lang::surface::{
        bytes_methods::BytesMethodId, clone_methods::CloneMethodId, float_methods::FloatMethodId,
        frozen_bytes_methods::FrozenBytesMethodId, iterator_methods::IteratorMethodId, list_methods::ListMethodId,
        option_methods::OptionMethodId, result_methods::ResultMethodId, set_methods::SetMethodId,
        string_methods::StringMethodId,
    };
    use crate::lang::types::numerics::{
        IntegerHelperFamily, IntegerHelperOperation, NumericResizeMethodId, NumericTypeId,
    };

    /// Protect function aliases and collection-method effects through their typed registry identities.
    #[test]
    fn builtin_registry_owns_aliases_and_method_effects() {
        assert_eq!(protected_binding_builtin("print"), Some(BuiltinFnId::Print));
        assert_eq!(protected_binding_builtin("println"), Some(BuiltinFnId::Print));
        assert_eq!(protected_binding_builtin("len"), None);
        assert!(BuiltinMethodId::List(ListMethodId::Append).changes_receiver());
        assert!(!BuiltinMethodId::List(ListMethodId::Contains).changes_receiver());
        assert!(BuiltinMethodId::Set(SetMethodId::Add).changes_receiver());
        let operations = [
            BuiltinMethodId::Str(StringMethodId::Encode),
            BuiltinMethodId::FrozenStr(StringMethodId::SplitWhitespace),
            BuiltinMethodId::Bytes(BytesMethodId::AsSlice),
            BuiltinMethodId::FrozenBytes(FrozenBytesMethodId::Decode),
            BuiltinMethodId::Iterator(IteratorMethodId::Take),
            BuiltinMethodId::Generator(IteratorMethodId::Collect),
            BuiltinMethodId::Option(OptionMethodId::IsSome),
            BuiltinMethodId::Result(ResultMethodId::Map),
            BuiltinMethodId::Clone(CloneMethodId::Clone),
            BuiltinMethodId::Float(NumericTypeId::F64, FloatMethodId::Sqrt),
            BuiltinMethodId::IntegerHelper(
                NumericTypeId::U8,
                IntegerHelperFamily::Checked,
                IntegerHelperOperation::Add,
            ),
            BuiltinMethodId::NumericResize(NumericTypeId::U8, NumericResizeMethodId::Try),
            BuiltinMethodId::ListIter,
        ];
        for operation in operations {
            assert_eq!(
                method_from_parts(operation.owner(), operation.member()),
                Some(operation)
            );
        }
    }
}
