//! String method surface vocabulary.
//!
//! These are the user-facing method names available on `str`/`FrozenStr` in the language surface.

use crate::lang::registry::{LangItemInfo, RFC, RfcId, Since, Stability};

/// Stable identifier for a string method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// `text.encode(encoding="utf-8")` returns the UTF-8 bytes of the text (#1668).
    Encode,
    /// Compiler-internal signed-bound slice operation selected for primitive `str` slicing.
    Slice,
}

/// One positional argument shape shared by the selected canonical string-helper subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedStringMethodArgumentKind {
    /// A runtime Incan `str` value.
    Str,
    /// A runtime Incan `list[str]` value.
    ListOfStr,
}

/// Canonical positional-call contract for one selected runtime string helper.
///
/// Parameter names deliberately do not appear here. The ordinary documented forms use positional arguments, and
/// assigning names would create keyword-call language semantics that this metadata does not yet establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedStringMethodSignature {
    /// Positional argument shapes in source order, excluding the string receiver.
    pub positional_arguments: &'static [SelectedStringMethodArgumentKind],
    /// Number of leading positional arguments that callers must supply.
    pub required_positional_arguments: usize,
}

impl StringMethodId {
    /// Return the positional-call contract for one string method admitted as a canonical helper operation.
    ///
    /// `None` deliberately keeps registered methods outside the selected helper subset from acquiring a direct
    /// execution contract just because they share the string-method vocabulary.
    pub const fn selected_helper_signature(self) -> Option<SelectedStringMethodSignature> {
        const NO_ARGUMENTS: &[SelectedStringMethodArgumentKind] = &[];
        const ONE_STR: &[SelectedStringMethodArgumentKind] = &[SelectedStringMethodArgumentKind::Str];
        const TWO_STRINGS: &[SelectedStringMethodArgumentKind] = &[
            SelectedStringMethodArgumentKind::Str,
            SelectedStringMethodArgumentKind::Str,
        ];
        const ONE_LIST_OF_STR: &[SelectedStringMethodArgumentKind] = &[SelectedStringMethodArgumentKind::ListOfStr];

        match self {
            Self::Upper | Self::Lower | Self::Strip | Self::Len => Some(SelectedStringMethodSignature {
                positional_arguments: NO_ARGUMENTS,
                required_positional_arguments: 0,
            }),
            Self::Replace => Some(SelectedStringMethodSignature {
                positional_arguments: TWO_STRINGS,
                required_positional_arguments: 2,
            }),
            Self::Join => Some(SelectedStringMethodSignature {
                positional_arguments: ONE_LIST_OF_STR,
                required_positional_arguments: 1,
            }),
            Self::Split => Some(SelectedStringMethodSignature {
                positional_arguments: ONE_STR,
                required_positional_arguments: 0,
            }),
            Self::Contains => Some(SelectedStringMethodSignature {
                positional_arguments: ONE_STR,
                required_positional_arguments: 1,
            }),
            Self::ToString
            | Self::SplitWhitespace
            | Self::StartsWith
            | Self::EndsWith
            | Self::IsEmpty
            | Self::Encode
            | Self::Slice => None,
        }
    }
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
    info(
        StringMethodId::Encode,
        "encode",
        &[],
        "Return the text encoded as bytes; only UTF-8 is supported (`encoding=\"utf-8\"` is the default).",
        RFC::_009,
        Since(0, 6),
    ),
];

const STRING_SLICE_METHOD: StringMethodInfo = info(
    StringMethodId::Slice,
    "__slice__",
    &[],
    "Return the primitive text slice between two checked signed bounds.",
    RFC::_009,
    Since(0, 1),
);

/// Resolve a string method spelling to its stable id.
pub fn from_str(name: &str) -> Option<StringMethodId> {
    super::from_str_impl(STRING_METHODS, name)
}

/// Return the canonical spelling for a string method.
pub fn as_str(id: StringMethodId) -> &'static str {
    match id {
        StringMethodId::Slice => STRING_SLICE_METHOD.canonical,
        _ => info_for(id).canonical,
    }
}

/// Return the full metadata entry for a string method.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
pub fn info_for(id: StringMethodId) -> &'static StringMethodInfo {
    match id {
        StringMethodId::Slice => &STRING_SLICE_METHOD,
        _ => super::info_for_impl(STRING_METHODS, id, "string method info missing"),
    }
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
