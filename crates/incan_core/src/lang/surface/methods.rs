//! Surface “method” vocabularies.
//!
//! This module groups the per-receiver method registries (e.g. `str` methods, `list` methods) into a single compilation
//! unit to reduce file sprawl, while keeping the public module structure re-exportable from `crate::lang::surface`.

use crate::lang::registry::LangItemInfo;

/// Resolve a spelling to a stable id for a registry table.
fn from_str_impl<Id: Copy>(items: &[LangItemInfo<Id>], name: &str) -> Option<Id> {
    if let Some(m) = items.iter().find(|m| m.canonical == name) {
        return Some(m.id);
    }
    items
        .iter()
        .find(|m| {
            let aliases: &[&str] = m.aliases;
            aliases.contains(&name)
        })
        .map(|m| m.id)
}

/// Return the registry metadata entry for a stable id.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
fn info_for_impl<Id: Copy + PartialEq>(
    items: &'static [LangItemInfo<Id>],
    id: Id,
    missing_msg: &'static str,
) -> &'static LangItemInfo<Id> {
    items
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("{missing_msg}"))
}

pub mod string_methods {
    //! String method surface vocabulary.
    //!
    //! These are the user-facing method names available on `str`/`FrozenStr` in the language surface.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a string method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum StringMethodId {
        Upper,
        Lower,
        Strip,
        Replace,
        Join,
        ToString,
        SplitWhitespace,
        Split,
        Contains,
        StartsWith,
        EndsWith,
        Len,
        IsEmpty,
    }

    pub type StringMethodInfo = LangItemInfo<StringMethodId>;

    /// Registry of all string methods.
    pub const STRING_METHODS: &[StringMethodInfo] = &[
        info(
            StringMethodId::Upper,
            "upper",
            &[],
            "Convert to uppercase.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Lower,
            "lower",
            &[],
            "Convert to lowercase.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Strip,
            "strip",
            &[],
            "Strip leading and trailing whitespace.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Replace,
            "replace",
            &[],
            "Replace occurrences of a substring.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Join,
            "join",
            &[],
            "Join an iterable/list of strings with this separator.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::ToString,
            "to_string",
            &[],
            "Return a string representation (identity for strings).",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::SplitWhitespace,
            "split_whitespace",
            &[],
            "Split on Unicode whitespace.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Split,
            "split",
            &[],
            "Split on a separator substring.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Contains,
            "contains",
            &[],
            "Return true if the substring occurs within the string.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::StartsWith,
            "startswith",
            &["starts_with"],
            "Return true if the string starts with a prefix.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::EndsWith,
            "endswith",
            &["ends_with"],
            "Return true if the string ends with a suffix.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::Len,
            "len",
            &[],
            "Return the length (in Unicode scalars).",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            StringMethodId::IsEmpty,
            "is_empty",
            &[],
            "Return true if the length is zero.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a string method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<StringMethodId> {
        super::from_str_impl(STRING_METHODS, name)
    }

    /// Return the canonical spelling for a string method.
    pub fn as_str(id: StringMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a string method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: StringMethodId) -> &'static StringMethodInfo {
        super::info_for_impl(STRING_METHODS, id, "string method info missing")
    }

    const fn info(
        id: StringMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> StringMethodInfo {
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
}

pub mod set_methods {
    //! Set method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a set method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum SetMethodId {
        Contains,
    }

    pub type SetMethodInfo = LangItemInfo<SetMethodId>;

    /// Registry of all set methods.
    pub const SET_METHODS: &[SetMethodInfo] = &[info(
        SetMethodId::Contains,
        "contains",
        &[],
        "Return true if the set contains a value.",
        RFC::_009,
        Since(0, 1),
    )];

    /// Resolve a set method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<SetMethodId> {
        super::from_str_impl(SET_METHODS, name)
    }

    /// Return the canonical spelling for a set method.
    pub fn as_str(id: SetMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a set method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: SetMethodId) -> &'static SetMethodInfo {
        super::info_for_impl(SET_METHODS, id, "set method info missing")
    }

    const fn info(
        id: SetMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> SetMethodInfo {
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
}

pub mod list_methods {
    //! List method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a list method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ListMethodId {
        Append,
        Extend,
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
            ListMethodId::Pop,
            "pop",
            &[],
            "Remove and return the last element. On an empty list, panics with `IndexError: pop from empty list` (Python-compatible).",
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
            "Remove and return the element at the given index.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            ListMethodId::Count,
            "count",
            &[],
            "Count occurrences of a value.",
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
}

pub mod dict_methods {
    //! Dict method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a dict method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum DictMethodId {
        Keys,
        Values,
        Get,
        Insert,
    }

    pub type DictMethodInfo = LangItemInfo<DictMethodId>;

    /// Registry of all dict methods.
    pub const DICT_METHODS: &[DictMethodInfo] = &[
        info(
            DictMethodId::Keys,
            "keys",
            &[],
            "Return an iterable/list of keys.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            DictMethodId::Values,
            "values",
            &[],
            "Return an iterable/list of values.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            DictMethodId::Get,
            "get",
            &[],
            "Get a value by key, optionally with a default.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            DictMethodId::Insert,
            "insert",
            &[],
            "Insert or overwrite a key/value pair.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a dict method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<DictMethodId> {
        super::from_str_impl(DICT_METHODS, name)
    }

    /// Return the canonical spelling for a dict method.
    pub fn as_str(id: DictMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a dict method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: DictMethodId) -> &'static DictMethodInfo {
        super::info_for_impl(DICT_METHODS, id, "dict method info missing")
    }

    const fn info(
        id: DictMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> DictMethodInfo {
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
}

pub mod frozen_set_methods {
    //! FrozenSet method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a frozen set method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FrozenSetMethodId {
        Len,
        IsEmpty,
        Contains,
    }

    pub type FrozenSetMethodInfo = LangItemInfo<FrozenSetMethodId>;

    /// Registry of all frozen set methods.
    pub const FROZEN_SET_METHODS: &[FrozenSetMethodInfo] = &[
        info(
            FrozenSetMethodId::Len,
            "len",
            &[],
            "Return the number of elements.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenSetMethodId::IsEmpty,
            "is_empty",
            &[],
            "Return true if the set is empty.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenSetMethodId::Contains,
            "contains",
            &[],
            "Return true if the set contains a value.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a frozen set method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<FrozenSetMethodId> {
        super::from_str_impl(FROZEN_SET_METHODS, name)
    }

    /// Return the canonical spelling for a frozen set method.
    pub fn as_str(id: FrozenSetMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a frozen set method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: FrozenSetMethodId) -> &'static FrozenSetMethodInfo {
        super::info_for_impl(FROZEN_SET_METHODS, id, "frozen set method info missing")
    }

    const fn info(
        id: FrozenSetMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> FrozenSetMethodInfo {
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
}

pub mod frozen_list_methods {
    //! FrozenList method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a frozen list method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FrozenListMethodId {
        Len,
        IsEmpty,
    }

    pub type FrozenListMethodInfo = LangItemInfo<FrozenListMethodId>;

    /// Registry of all frozen list methods.
    pub const FROZEN_LIST_METHODS: &[FrozenListMethodInfo] = &[
        info(
            FrozenListMethodId::Len,
            "len",
            &[],
            "Return the number of elements.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenListMethodId::IsEmpty,
            "is_empty",
            &[],
            "Return true if the list is empty.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a frozen list method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<FrozenListMethodId> {
        super::from_str_impl(FROZEN_LIST_METHODS, name)
    }

    /// Return the canonical spelling for a frozen list method.
    pub fn as_str(id: FrozenListMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a frozen list method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: FrozenListMethodId) -> &'static FrozenListMethodInfo {
        super::info_for_impl(FROZEN_LIST_METHODS, id, "frozen list method info missing")
    }

    const fn info(
        id: FrozenListMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> FrozenListMethodInfo {
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
}

pub mod frozen_dict_methods {
    //! FrozenDict method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a frozen dict method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FrozenDictMethodId {
        Len,
        IsEmpty,
        ContainsKey,
    }

    pub type FrozenDictMethodInfo = LangItemInfo<FrozenDictMethodId>;

    /// Registry of all frozen dict methods.
    pub const FROZEN_DICT_METHODS: &[FrozenDictMethodInfo] = &[
        info(
            FrozenDictMethodId::Len,
            "len",
            &[],
            "Return the number of entries.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenDictMethodId::IsEmpty,
            "is_empty",
            &[],
            "Return true if the dict is empty.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenDictMethodId::ContainsKey,
            "contains_key",
            &[],
            "Return true if the dict contains a key.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a frozen dict method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<FrozenDictMethodId> {
        super::from_str_impl(FROZEN_DICT_METHODS, name)
    }

    /// Return the canonical spelling for a frozen dict method.
    pub fn as_str(id: FrozenDictMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a frozen dict method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: FrozenDictMethodId) -> &'static FrozenDictMethodInfo {
        super::info_for_impl(FROZEN_DICT_METHODS, id, "frozen dict method info missing")
    }

    const fn info(
        id: FrozenDictMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> FrozenDictMethodInfo {
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
}

pub mod frozen_bytes_methods {
    //! FrozenBytes method surface vocabulary.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a frozen bytes method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FrozenBytesMethodId {
        Len,
        IsEmpty,
    }

    pub type FrozenBytesMethodInfo = LangItemInfo<FrozenBytesMethodId>;

    /// Registry of all frozen bytes methods.
    pub const FROZEN_BYTES_METHODS: &[FrozenBytesMethodInfo] = &[
        info(
            FrozenBytesMethodId::Len,
            "len",
            &[],
            "Return the number of bytes.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FrozenBytesMethodId::IsEmpty,
            "is_empty",
            &[],
            "Return true if the byte string is empty.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a frozen bytes method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<FrozenBytesMethodId> {
        super::from_str_impl(FROZEN_BYTES_METHODS, name)
    }

    /// Return the canonical spelling for a frozen bytes method.
    pub fn as_str(id: FrozenBytesMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a frozen bytes method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: FrozenBytesMethodId) -> &'static FrozenBytesMethodInfo {
        super::info_for_impl(FROZEN_BYTES_METHODS, id, "frozen bytes method info missing")
    }

    const fn info(
        id: FrozenBytesMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> FrozenBytesMethodInfo {
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
}

pub mod float_methods {
    //! Float method surface vocabulary.
    //!
    //! These are the user-facing methods that the typechecker treats as available on `float`.

    use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

    /// Stable identifier for a float method.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum FloatMethodId {
        // f64-like math
        Sqrt,
        Abs,
        Floor,
        Ceil,
        Round,
        Sin,
        Cos,
        Tan,
        Exp,
        Ln,
        Log2,
        Log10,

        // predicates
        IsNan,
        IsInfinite,
        IsFinite,

        // powers
        Powi,
        Powf,
    }

    pub type FloatMethodInfo = LangItemInfo<FloatMethodId>;

    /// Registry of all float methods.
    pub const FLOAT_METHODS: &[FloatMethodInfo] = &[
        info(FloatMethodId::Sqrt, "sqrt", &[], "Square root.", RFC::_009, Since(0, 1)),
        info(
            FloatMethodId::Abs,
            "abs",
            &[],
            "Absolute value.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Floor,
            "floor",
            &[],
            "Round down to the nearest integer (as float).",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Ceil,
            "ceil",
            &[],
            "Round up to the nearest integer (as float).",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Round,
            "round",
            &[],
            "Round to the nearest integer (as float).",
            RFC::_009,
            Since(0, 1),
        ),
        info(FloatMethodId::Sin, "sin", &[], "Sine.", RFC::_009, Since(0, 1)),
        info(FloatMethodId::Cos, "cos", &[], "Cosine.", RFC::_009, Since(0, 1)),
        info(FloatMethodId::Tan, "tan", &[], "Tangent.", RFC::_009, Since(0, 1)),
        info(
            FloatMethodId::Exp,
            "exp",
            &[],
            "Exponentiation (e^x).",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Ln,
            "ln",
            &[],
            "Natural logarithm.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Log2,
            "log2",
            &[],
            "Base-2 logarithm.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Log10,
            "log10",
            &[],
            "Base-10 logarithm.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::IsNan,
            "is_nan",
            &[],
            "Return true if this value is NaN.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::IsInfinite,
            "is_infinite",
            &[],
            "Return true if this value is ±infinity.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::IsFinite,
            "is_finite",
            &[],
            "Return true if this value is finite.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Powi,
            "powi",
            &[],
            "Raise to an integer power.",
            RFC::_009,
            Since(0, 1),
        ),
        info(
            FloatMethodId::Powf,
            "powf",
            &[],
            "Raise to a float power.",
            RFC::_009,
            Since(0, 1),
        ),
    ];

    /// Resolve a float method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<FloatMethodId> {
        super::from_str_impl(FLOAT_METHODS, name)
    }

    /// Return the canonical spelling for a float method.
    pub fn as_str(id: FloatMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a float method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: FloatMethodId) -> &'static FloatMethodInfo {
        super::info_for_impl(FLOAT_METHODS, id, "float method info missing")
    }

    const fn info(
        id: FloatMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> FloatMethodInfo {
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
}

pub mod option_methods {
    use super::LangItemInfo;
    use crate::lang::registry::{RFC, RfcId, Since, Stability};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum OptionMethodId {
        Copied,
        UnwrapOr,
        Unwrap,
    }

    pub type OptionMethodInfo = LangItemInfo<OptionMethodId>;

    pub const OPTION_METHODS: &[OptionMethodInfo] = &[
        info(
            OptionMethodId::Copied,
            "copied",
            &[],
            "Copy from Option[&T] to Option[T] when T: Copy.",
            RFC::_000,
            Since(0, 1),
        ),
        info(
            OptionMethodId::UnwrapOr,
            "unwrap_or",
            &[],
            "Return the contained value or a default.",
            RFC::_000,
            Since(0, 1),
        ),
        info(
            OptionMethodId::Unwrap,
            "unwrap",
            &[],
            "Return the contained value or panic.",
            RFC::_000,
            Since(0, 1),
        ),
    ];

    pub fn from_str(name: &str) -> Option<OptionMethodId> {
        super::from_str_impl(OPTION_METHODS, name)
    }

    pub fn as_str(id: OptionMethodId) -> &'static str {
        info_for(id).canonical
    }

    pub fn info_for(id: OptionMethodId) -> &'static OptionMethodInfo {
        super::info_for_impl(OPTION_METHODS, id, "option method info missing")
    }

    const fn info(
        id: OptionMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> OptionMethodInfo {
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
}

pub mod result_methods {
    //! Result method surface vocabulary.

    use super::LangItemInfo;
    use crate::lang::registry::{RFC, RfcId, Since, Stability};

    /// Stable identifier for an RFC 070 `Result[T, E]` combinator.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ResultMethodId {
        Map,
        MapErr,
        AndThen,
        OrElse,
        Inspect,
        InspectErr,
    }

    pub type ResultMethodInfo = LangItemInfo<ResultMethodId>;

    pub const RESULT_METHODS: &[ResultMethodInfo] = &[
        info(
            ResultMethodId::Map,
            "map",
            &[],
            "Transform an Ok payload while preserving Err.",
            RFC::_070,
            Since(0, 3),
        ),
        info(
            ResultMethodId::MapErr,
            "map_err",
            &[],
            "Transform an Err payload while preserving Ok.",
            RFC::_070,
            Since(0, 3),
        ),
        info(
            ResultMethodId::AndThen,
            "and_then",
            &[],
            "Chain a Result-returning operation from an Ok payload.",
            RFC::_070,
            Since(0, 3),
        ),
        info(
            ResultMethodId::OrElse,
            "or_else",
            &[],
            "Recover or remap through a Result-returning operation from an Err payload.",
            RFC::_070,
            Since(0, 3),
        ),
        info(
            ResultMethodId::Inspect,
            "inspect",
            &[],
            "Observe an Ok payload by implicit borrow while preserving the original Result.",
            RFC::_070,
            Since(0, 3),
        ),
        info(
            ResultMethodId::InspectErr,
            "inspect_err",
            &[],
            "Observe an Err payload by implicit borrow while preserving the original Result.",
            RFC::_070,
            Since(0, 3),
        ),
    ];

    /// Resolve a result method spelling to its stable id.
    pub fn from_str(name: &str) -> Option<ResultMethodId> {
        super::from_str_impl(RESULT_METHODS, name)
    }

    /// Return the canonical spelling for a result method.
    pub fn as_str(id: ResultMethodId) -> &'static str {
        info_for(id).canonical
    }

    /// Return the full metadata entry for a result method.
    ///
    /// ## Panics
    /// - If the registry is missing an entry for `id` (this indicates a programming error).
    pub fn info_for(id: ResultMethodId) -> &'static ResultMethodInfo {
        super::info_for_impl(RESULT_METHODS, id, "result method info missing")
    }

    /// Construct result method metadata for the static registry.
    const fn info(
        id: ResultMethodId,
        canonical: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        introduced_in_rfc: RfcId,
        since: Since,
    ) -> ResultMethodInfo {
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
}
